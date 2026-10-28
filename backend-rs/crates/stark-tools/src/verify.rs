//! Minimal acceptance-criteria evaluation for group work items.
//!
//! Python parity (agents/group/criteria.py) judges every item against its own
//! acceptance criteria. This is the deterministic subset: file/dir existence,
//! minimum size, regex content matches, and glob presence — all resolved
//! strictly under the project root. Anything else (`manual`, `llm`,
//! `command`, free-form strings) cannot be machine-checked here:
//! `manual` entries route the item to owner approval, the rest are recorded
//! as assumed-met so verification never invents a failure it cannot prove.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CriterionStatus {
    Pass,
    Fail,
    NeedsHuman,
    Assumed,
}

#[derive(Debug, Clone)]
pub struct CriterionVerdict {
    pub status: CriterionStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct AcceptanceVerdict {
    pub passed: Vec<CriterionVerdict>,
    pub failed: Vec<CriterionVerdict>,
    pub needs_human: Vec<CriterionVerdict>,
    pub assumed: Vec<CriterionVerdict>,
}

impl AcceptanceVerdict {
    pub fn ok(&self) -> bool {
        self.failed.is_empty() && self.needs_human.is_empty()
    }

    pub fn summary(&self) -> String {
        let total =
            self.passed.len() + self.failed.len() + self.needs_human.len() + self.assumed.len();
        if total == 0 {
            return "no acceptance criteria recorded".to_string();
        }
        let mut parts = vec![format!("{}/{} criteria verified", self.passed.len(), total)];
        for verdict in self.failed.iter().chain(self.needs_human.iter()) {
            parts.push(verdict.detail.clone());
        }
        if !self.assumed.is_empty() {
            parts.push(format!(
                "{} not machine-checkable (assumed met)",
                self.assumed.len()
            ));
        }
        parts.join("; ")
    }
}

/// Resolve `relative` strictly under `root`. Rejects absolute paths and any
/// path that escapes the project root.
/// Resolve `relative` strictly under `root`. Rejects absolute paths and any
/// path that escapes the project root. The candidate is built from the
/// canonical root so `\\?\`-style Windows prefixes cannot cause false escapes.
fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.trim().is_empty() {
        return Err("criterion has no path".to_string());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        return Err(format!("absolute path not allowed: {relative:?}"));
    }
    let canon = canonical_root(root);
    let mut resolved = canon.clone();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() || (resolved != canon && !resolved.starts_with(&canon)) {
                    return Err(format!("path escapes the project root: {relative:?}"));
                }
            }
            Component::Normal(part) => resolved.push(part),
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!("absolute path not allowed: {relative:?}"));
            }
        }
    }
    if resolved != canon && !resolved.starts_with(&canon) {
        return Err(format!("path escapes the project root: {relative:?}"));
    }
    Ok(resolved)
}

fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

fn criterion_kind(criterion: &Value) -> String {
    criterion
        .get("kind")
        .or_else(|| criterion.get("check"))
        .or_else(|| criterion.get("type"))
        .or_else(|| criterion.get("op"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn criterion_path(criterion: &Value) -> String {
    criterion
        .get("path")
        .or_else(|| criterion.get("file"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn pass(detail: String) -> CriterionVerdict {
    CriterionVerdict {
        status: CriterionStatus::Pass,
        detail,
    }
}

fn fail(detail: String) -> CriterionVerdict {
    CriterionVerdict {
        status: CriterionStatus::Fail,
        detail,
    }
}

fn evaluate_object(root: &Path, criterion: &Value) -> CriterionVerdict {
    match criterion_kind(criterion).as_str() {
        "manual" | "human" | "approval" => CriterionVerdict {
            status: CriterionStatus::NeedsHuman,
            detail: format!(
                "manual verification required: {}",
                criterion
                    .get("question")
                    .or_else(|| criterion.get("description"))
                    .or_else(|| criterion.get("detail"))
                    .and_then(Value::as_str)
                    .unwrap_or("owner must confirm")
            ),
        },
        "file_exists" | "file" | "exists" => match safe_path(root, &criterion_path(criterion)) {
            Err(error) => fail(error),
            Ok(path) if path.is_file() => {
                pass(format!("file exists: {}", criterion_path(criterion)))
            }
            Ok(_) => fail(format!("missing file: {}", criterion_path(criterion))),
        },
        "dir_exists" | "dir" | "directory" => match safe_path(root, &criterion_path(criterion)) {
            Err(error) => fail(error),
            Ok(path) if path.is_dir() => pass(format!("dir exists: {}", criterion_path(criterion))),
            Ok(_) => fail(format!("missing dir: {}", criterion_path(criterion))),
        },
        "min_bytes" | "min_size" | "size" => {
            let want: u64 = criterion
                .get("bytes")
                .or_else(|| criterion.get("size"))
                .or_else(|| criterion.get("min"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            match safe_path(root, &criterion_path(criterion)) {
                Err(error) => fail(error),
                Ok(path) => match std::fs::metadata(&path) {
                    Ok(meta) if meta.is_file() && meta.len() >= want => pass(format!(
                        "size ok: {} ({} >= {} bytes)",
                        criterion_path(criterion),
                        meta.len(),
                        want
                    )),
                    Ok(meta) if meta.is_file() => fail(format!(
                        "too small: {} ({} < {} bytes)",
                        criterion_path(criterion),
                        meta.len(),
                        want
                    )),
                    _ => fail(format!("missing file: {}", criterion_path(criterion))),
                },
            }
        }
        "regex_match" | "regex" | "content_match" | "contains" => {
            let pattern = criterion
                .get("pattern")
                .or_else(|| criterion.get("regex"))
                .or_else(|| criterion.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if pattern.is_empty() {
                return fail("regex criterion has no pattern".to_string());
            }
            let regex = match regex::Regex::new(pattern) {
                Ok(regex) => regex,
                Err(error) => return fail(format!("bad regex {pattern:?}: {error}")),
            };
            match safe_path(root, &criterion_path(criterion)) {
                Err(error) => fail(error),
                Ok(path) => match std::fs::read_to_string(&path) {
                    Ok(content) if regex.is_match(&content) => {
                        pass(format!("pattern matched in {}", criterion_path(criterion)))
                    }
                    Ok(_) => fail(format!(
                        "pattern not found in {}",
                        criterion_path(criterion)
                    )),
                    Err(_) => fail(format!("unreadable file: {}", criterion_path(criterion))),
                },
            }
        }
        "glob" | "glob_exists" | "files_match" => {
            let pattern = criterion
                .get("pattern")
                .or_else(|| criterion.get("glob"))
                .or_else(|| criterion.get("path"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if pattern.is_empty() {
                return fail("glob criterion has no pattern".to_string());
            }
            if pattern.trim_start().starts_with('/')
                || pattern.contains("..")
                || pattern
                    .split('/')
                    .any(|part| part == "~" || part.contains(':'))
            {
                return fail(format!("unsafe glob pattern: {pattern:?}"));
            }
            match globset::Glob::new(pattern) {
                Err(error) => fail(format!("bad glob {pattern:?}: {error}")),
                Ok(glob) => {
                    let matcher = glob.compile_matcher();
                    let mut matched = false;
                    let mut stack = vec![root.to_path_buf()];
                    while let Some(dir) = stack.pop() {
                        let Ok(entries) = std::fs::read_dir(&dir) else {
                            continue;
                        };
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                stack.push(path);
                                continue;
                            }
                            let relative = path.strip_prefix(root).unwrap_or(path.as_path());
                            if matcher.is_match(relative) || matcher.is_match(&path) {
                                matched = true;
                                break;
                            }
                        }
                        if matched {
                            break;
                        }
                    }
                    if matched {
                        pass(format!("glob matched: {pattern}"))
                    } else {
                        fail(format!("glob matched nothing: {pattern}"))
                    }
                }
            }
        }
        other => CriterionVerdict {
            status: CriterionStatus::Assumed,
            detail: format!("{other} criterion is not machine-checkable (assumed met)"),
        },
    }
}

/// Evaluate an item's `acceptance` value (array of criterion objects and/or
/// free-form strings) against the project root.
pub fn evaluate_acceptance(project_root: &Path, acceptance: &Value) -> AcceptanceVerdict {
    let mut verdict = AcceptanceVerdict::default();
    let Some(items) = acceptance.as_array() else {
        return verdict;
    };
    for criterion in items {
        if criterion.is_string() {
            verdict.assumed.push(CriterionVerdict {
                status: CriterionStatus::Assumed,
                detail: format!(
                    "free-form criterion assumed met: {}",
                    criterion
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>()
                ),
            });
            continue;
        }
        if !criterion.is_object() {
            continue;
        }
        match evaluate_object(project_root, criterion) {
            CriterionVerdict {
                status: CriterionStatus::Pass,
                detail,
            } => verdict.passed.push(CriterionVerdict {
                status: CriterionStatus::Pass,
                detail,
            }),
            CriterionVerdict {
                status: CriterionStatus::Fail,
                detail,
            } => verdict.failed.push(CriterionVerdict {
                status: CriterionStatus::Fail,
                detail,
            }),
            CriterionVerdict {
                status: CriterionStatus::NeedsHuman,
                detail,
            } => verdict.needs_human.push(CriterionVerdict {
                status: CriterionStatus::NeedsHuman,
                detail,
            }),
            assumed => verdict.assumed.push(assumed),
        }
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn project() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn write(root: &Path, name: &str, content: &str) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn file_dir_and_size_checks() {
        let dir = project();
        write(dir.path(), "out/report.md", "hello world");
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "file_exists", "path": "out/report.md"},
                {"kind": "dir_exists", "path": "logs"},
                {"kind": "min_bytes", "path": "out/report.md", "bytes": 5},
            ]),
        );
        assert!(verdict.ok());
        assert_eq!(verdict.passed.len(), 3);
    }

    #[test]
    fn missing_and_small_files_fail() {
        let dir = project();
        write(dir.path(), "tiny.txt", "x");
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "file_exists", "path": "nope.txt"},
                {"kind": "min_bytes", "path": "tiny.txt", "bytes": 100},
            ]),
        );
        assert!(!verdict.ok());
        assert_eq!(verdict.failed.len(), 2);
    }

    #[test]
    fn regex_and_glob_checks() {
        let dir = project();
        write(dir.path(), "src/main.rs", "fn main() { println!(\"hi\"); }");
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "regex_match", "path": "src/main.rs", "pattern": "fn main"},
                {"kind": "glob", "pattern": "src/*.rs"},
            ]),
        );
        assert!(verdict.ok());
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "regex_match", "path": "src/main.rs", "pattern": "fn missing"},
                {"kind": "glob", "pattern": "src/*.py"},
            ]),
        );
        assert!(!verdict.ok());
        assert_eq!(verdict.failed.len(), 2);
    }

    #[test]
    fn traversal_and_absolute_paths_fail() {
        let dir = project();
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "file_exists", "path": "../escape.txt"},
                {"kind": "file_exists", "path": "/etc/hostname"},
            ]),
        );
        assert!(!verdict.ok());
        assert_eq!(verdict.failed.len(), 2);
    }

    #[test]
    fn manual_routes_to_human_and_strings_are_assumed() {
        let dir = project();
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "manual", "question": "Does the UI look right?"},
                "Looks good to stakeholders",
            ]),
        );
        assert!(!verdict.ok());
        assert_eq!(verdict.needs_human.len(), 1);
        assert_eq!(verdict.assumed.len(), 1);
        assert!(verdict.failed.is_empty());
    }

    #[test]
    fn empty_acceptance_is_vacuous_with_note() {
        let dir = project();
        let verdict = evaluate_acceptance(dir.path(), &json!([]));
        assert!(verdict.ok());
        assert!(verdict.summary().contains("no acceptance criteria"));
    }

    #[test]
    fn bad_regex_and_glob_are_failures_not_panics() {
        let dir = project();
        let verdict = evaluate_acceptance(
            dir.path(),
            &json!([
                {"kind": "regex_match", "path": "x.txt", "pattern": "(unclosed"},
                {"kind": "glob", "pattern": ""},
            ]),
        );
        assert!(!verdict.ok());
        assert_eq!(verdict.failed.len(), 2);
    }
}
