//! Git worktree isolation for spawned agents.
//!
//! A spawned write-capable agent gets its own checkout on its own branch.
//! The parent reviews the diff and merges explicitly. The live tree never
//! sees unreviewed child writes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub root: PathBuf,
    pub subdir: PathBuf,
    pub head: String,
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub repo_root: PathBuf,
    pub subdir: PathBuf,
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("git failed: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!("git {} failed: {message}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_with_timeout(root: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    // Git worktree operations are local and fast; a blocking call with a
    // watchdog thread keeps the async runtime responsive.
    let root = root.to_path_buf();
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let label = owned.join(" ");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        tx.send(git(&root, &refs)).ok();
    });
    rx.recv_timeout(timeout)
        .map_err(|_| format!("git {label} timed out"))?
}

pub fn detect_repo(workspace: &Path) -> Result<RepoInfo, String> {
    let root = git(workspace, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| "not a git repository".to_string())?;
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap_or_else(|_| "HEAD".to_string());
    let subdir = workspace
        .strip_prefix(&root)
        .map(PathBuf::from)
        .unwrap_or_default();
    Ok(RepoInfo { root, subdir, head })
}

fn worktree_root(harness_root: &Path, repo_root: &Path, agent_id: &str) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let short = agent_id.strip_prefix("agent-").unwrap_or(agent_id);
    let short: String = short.chars().take(12).collect();
    harness_root
        .join(".deep_agents")
        .join("worktrees")
        .join(format!("{repo_name}-{short}"))
}

pub fn create(
    harness_root: &Path,
    workspace: &Path,
    agent_id: &str,
) -> Result<WorktreeInfo, String> {
    let repo = detect_repo(workspace)?;
    let short: String = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let agent_short: String = agent_id
        .strip_prefix("agent-")
        .unwrap_or(agent_id)
        .chars()
        .take(8)
        .collect();
    let branch = format!("stark/agent-{agent_short}-{short}");
    let path = worktree_root(harness_root, &repo.root, agent_id);
    if path.exists() {
        return Err(format!("worktree path already exists: {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create worktree dir: {error}"))?;
    }
    run_with_timeout(
        &repo.root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            "HEAD",
        ],
        Duration::from_secs(60),
    )?;
    Ok(WorktreeInfo {
        path,
        branch,
        base: repo.head,
        repo_root: repo.root,
        subdir: repo.subdir,
    })
}

pub fn effective_workspace(info: &WorktreeInfo) -> PathBuf {
    if info.subdir.as_os_str().is_empty() {
        info.path.clone()
    } else {
        info.path.join(&info.subdir)
    }
}

pub fn diff(path: &Path, base: &str) -> Result<Value, String> {
    let stat = git(path, &["diff", "--stat", &format!("{base}..HEAD")]).unwrap_or_default();
    let names = git(path, &["diff", "--name-status", &format!("{base}..HEAD")]).unwrap_or_default();
    let files: Vec<Value> = names
        .lines()
        .map(|line| {
            let mut parts = line.splitn(2, '\t');
            json!({
                "status": parts.next().unwrap_or("").trim(),
                "path": parts.next().unwrap_or("").trim(),
            })
        })
        .filter(|file| !file["path"].as_str().unwrap_or("").is_empty())
        .collect();
    Ok(json!({"stat": stat, "files": files, "base": base}))
}

pub fn commit_changes(info: &WorktreeInfo, message: &str) -> Result<Value, String> {
    let status = git(&info.path, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(json!({"committed": false, "stat": ""}));
    }
    // Stage everything except agent metadata. The `:!.deep_agents` pathspec
    // keeps checkpoints/snapshots out of the reviewed diff; fall back to
    // add-all plus reset on servers with an older git.
    if git(&info.path, &["add", "-A", "--", ":!.deep_agents"]).is_err() {
        git(&info.path, &["add", "-A"])?;
        let _ = git(&info.path, &["reset", "-q", ".deep_agents"]);
    }
    let status = git(&info.path, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(json!({"committed": false, "stat": ""}));
    }
    let message = if message.trim().is_empty() {
        "stark agent work".to_string()
    } else {
        message.to_string()
    };
    // Commit may fail on identity; use a local fallback identity.
    let commit = git(
        &info.path,
        &[
            "-c",
            "user.name=Stark",
            "-c",
            "user.email=stark@localhost",
            "commit",
            "-m",
            &message,
        ],
    );
    commit?;
    let stat = git(&info.path, &["show", "--stat", "--oneline", "HEAD"]).unwrap_or_default();
    Ok(json!({"committed": true, "stat": stat}))
}

fn is_clean(path: &Path) -> bool {
    git(path, &["status", "--porcelain"])
        .map(|status| status.trim().is_empty())
        .unwrap_or(false)
}

pub fn merge(info: &WorktreeInfo) -> Result<Value, String> {
    if info.repo_root.as_os_str().is_empty() {
        return Err("agent has no repository".to_string());
    }
    if !is_clean(&info.repo_root) {
        return Err("target tree has uncommitted changes; commit or stash first".to_string());
    }
    // Ensure the branch is fully committed before merging.
    let _ = commit_changes(info, "stark agent work")?;
    let merge = git(
        &info.repo_root,
        &[
            "merge",
            "--no-ff",
            "-m",
            &format!("Merge {}", info.branch),
            &info.branch,
        ],
    );
    if let Err(error) = merge {
        let _ = git(&info.repo_root, &["merge", "--abort"]);
        return Err(format!(
            "merge conflict; worktree kept for manual resolution: {error}"
        ));
    }
    remove(&info.repo_root, &info.path, &info.branch)?;
    Ok(json!({"merged": true, "branch": info.branch}))
}

pub fn discard(repo_root: &Path, path: &Path, branch: &str) -> Result<Value, String> {
    remove(repo_root, path, branch)?;
    Ok(json!({"discarded": true, "branch": branch}))
}

fn remove(repo_root: &Path, path: &Path, branch: &str) -> Result<(), String> {
    let _ = git(
        repo_root,
        &["worktree", "remove", "--force", &path.to_string_lossy()],
    );
    let _ = git(repo_root, &["worktree", "prune"]);
    if !branch.trim().is_empty() {
        let _ = git(repo_root, &["branch", "-D", branch]);
    }
    Ok(())
}

pub fn reconcile(harness_root: &Path, agents: &[stark_store::AgentRecord]) -> Vec<String> {
    let mut notes = Vec::new();
    for agent in agents {
        if agent.worktree_path.trim().is_empty()
            || agent.worktree_state == "merged"
            || agent.worktree_state == "discarded"
        {
            continue;
        }
        let path = PathBuf::from(&agent.worktree_path);
        if !path.exists() && agent.worktree_state == "active" {
            notes.push(format!("{}: worktree missing", agent.id));
        }
    }
    let _ = harness_root;
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    #[test]
    fn worktree_create_merge_roundtrip() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(git(&repo, &["init", "-q"]).is_ok());
        assert!(git(&repo, &["config", "user.name", "Test"]).is_ok());
        assert!(git(&repo, &["config", "user.email", "test@test"]).is_ok());
        std::fs::write(repo.join("a.txt"), "one").unwrap();
        assert!(git(&repo, &["add", "-A"]).is_ok());
        assert!(git(&repo, &["commit", "-qm", "init"]).is_ok());

        let harness = dir.path().join("harness");
        std::fs::create_dir_all(&harness).unwrap();
        let info = create(&harness, &repo, "agent-test123456").expect("create worktree");
        std::fs::write(info.path.join("a.txt"), "two").unwrap();
        commit_changes(&info, "change a").unwrap();
        // Main tree unchanged before merge.
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one");
        merge(&info).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "two");
        assert!(!info.path.exists());
    }

    #[test]
    fn worktree_conflict_aborts_cleanly() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(git(&repo, &["init", "-q"]).is_ok());
        assert!(git(&repo, &["config", "user.name", "Test"]).is_ok());
        assert!(git(&repo, &["config", "user.email", "test@test"]).is_ok());
        std::fs::write(repo.join("a.txt"), "base").unwrap();
        assert!(git(&repo, &["add", "-A"]).is_ok());
        assert!(git(&repo, &["commit", "-qm", "init"]).is_ok());

        let harness = dir.path().join("harness");
        std::fs::create_dir_all(&harness).unwrap();
        let info = create(&harness, &repo, "agent-conflict1").expect("create");
        std::fs::write(info.path.join("a.txt"), "child").unwrap();
        commit_changes(&info, "child change").unwrap();
        std::fs::write(repo.join("a.txt"), "parent").unwrap();
        assert!(git(&repo, &["add", "-A"]).is_ok());
        assert!(git(&repo, &["commit", "-qm", "parent"]).is_ok());
        let result = merge(&info);
        assert!(result.is_err());
        // Worktree survives a conflict.
        assert!(info.path.exists());
        let _ = discard(&info.repo_root, &info.path, &info.branch);
    }

    #[test]
    fn non_git_workspace_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_repo(dir.path()).is_err());
    }
}
