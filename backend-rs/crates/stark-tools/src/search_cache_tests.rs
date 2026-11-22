//! Warm-index tests. Stark layer only — not part of the Codex-verbatim
//! `search_tests.rs` suite, which must stay an exact port.

use super::*;
use pretty_assertions::assert_eq;
use std::time::Instant;

fn no_ignore_opts() -> FileSearchOptions {
    FileSearchOptions {
        respect_gitignore: false,
        ..Default::default()
    }
}

fn write_tree(dir: &std::path::Path, files: &[&str]) {
    for name in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, format!("contents of {name}\n")).unwrap();
    }
}

#[test]
fn warm_matches_cold() {
    let dir = tempfile::tempdir().unwrap();
    write_tree(dir.path(), &["alpha.txt", "beta.rs", "gamma/notes.md"]);
    let roots = vec![dir.path().to_path_buf()];

    let cold = run("alpha", roots.clone(), no_ignore_opts(), None).unwrap();
    let warm = run_warm("alpha", roots, no_ignore_opts(), None).unwrap();

    assert_eq!(warm.matches, cold.matches);
    assert_eq!(warm.total_match_count, cold.total_match_count);
    assert!(!warm.matches.is_empty());
}

#[test]
fn warm_entries_refill_after_dirty() {
    let dir = tempfile::tempdir().unwrap();
    write_tree(dir.path(), &["a.txt"]);

    let first = warm_entries(dir.path(), &[], false, 2);
    assert!(first.iter().any(|(path, _)| path == "a.txt"));
    assert!(!first.iter().any(|(path, _)| path == "b.txt"));

    // New file is invisible until the slot is dirtied (stale cache).
    write_tree(dir.path(), &["b.txt"]);
    mark_dirty_for_tests(dir.path(), &[], false);
    let second = warm_entries(dir.path(), &[], false, 2);
    assert!(second.iter().any(|(path, _)| path == "a.txt"));
    assert!(second.iter().any(|(path, _)| path == "b.txt"));
}

#[test]
fn watcher_marks_dirty_on_fs_event() {
    let dir = tempfile::tempdir().unwrap();
    write_tree(dir.path(), &["seed.txt"]);

    // First call fills the cache and arms the recursive watcher.
    let _ = warm_entries(dir.path(), &[], false, 2);

    write_tree(dir.path(), &["arrived.txt"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = warm_entries(dir.path(), &[], false, 2);
        if entries.iter().any(|(path, _)| path == "arrived.txt") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "watcher did not mark the slot dirty within 10s"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn respect_gitignore_keys_are_separate() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    write_tree(dir.path(), &["ignored.txt", "kept.txt"]);

    let inclusive = warm_entries(dir.path(), &[], false, 2);
    assert!(inclusive.iter().any(|(path, _)| path == "ignored.txt"));

    let respecting = warm_entries(dir.path(), &[], true, 2);
    assert!(!respecting.iter().any(|(path, _)| path == "ignored.txt"));
    assert!(respecting.iter().any(|(path, _)| path == "kept.txt"));
}

#[test]
fn run_warm_multi_root_falls_back_to_cold() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_tree(first.path(), &["apple.txt"]);
    write_tree(second.path(), &["apple-pie.txt"]);
    let roots = vec![first.path().to_path_buf(), second.path().to_path_buf()];

    let cold = run("apple", roots.clone(), no_ignore_opts(), None).unwrap();
    let warm = run_warm("apple", roots, no_ignore_opts(), None).unwrap();

    assert_eq!(warm.matches, cold.matches);
    assert_eq!(warm.total_match_count, cold.total_match_count);
    assert!(!warm.matches.is_empty());
}
