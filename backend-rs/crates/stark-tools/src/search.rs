use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde_json::Value;

use crate::paths::{relative_display, resolve_workspace_path};
use crate::{ToolContext, ToolOutcome};

const DEFAULT_MAX_MATCHES: usize = 100;
const MAX_MATCHES: usize = 1000;

struct FileFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl FileFilter {
    fn parse(spec: &str) -> Result<Self, String> {
        let mut include_builder = GlobSetBuilder::new();
        let mut exclude_builder = GlobSetBuilder::new();
        let mut include_count = 0;
        let mut exclude_count = 0;

        for raw in spec.split(',').map(str::trim).filter(|raw| !raw.is_empty()) {
            let (exclude, pattern) = match raw.strip_prefix('!') {
                Some(pattern) => (true, pattern),
                None => (false, raw),
            };
            let glob = Glob::new(pattern).map_err(|error| error.to_string())?;
            if exclude {
                exclude_builder.add(glob);
                exclude_count += 1;
            } else {
                include_builder.add(glob);
                include_count += 1;
            }
        }

        Ok(Self {
            include: if include_count == 0 {
                None
            } else {
                Some(include_builder.build().map_err(|error| error.to_string())?)
            },
            exclude: if exclude_count == 0 {
                None
            } else {
                Some(exclude_builder.build().map_err(|error| error.to_string())?)
            },
        })
    }

    fn matches(&self, rel: &str) -> bool {
        if let Some(include) = &self.include
            && !include.is_match(rel)
        {
            return false;
        }
        if let Some(exclude) = &self.exclude
            && exclude.is_match(rel)
        {
            return false;
        }
        true
    }
}

pub async fn search_code(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let patterns: Vec<String> =
        if let Some(patterns) = args.get("patterns").and_then(Value::as_array) {
            patterns
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        } else if let Some(pattern) = args.get("pattern").and_then(Value::as_str) {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        };
    if patterns.is_empty() {
        return ToolOutcome::err("Error: search_code requires pattern or patterns");
    }

    let raw_path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = match resolve_workspace_path(&ctx.workspace, raw_path) {
        Ok(root) => root,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if !root.exists() {
        return ToolOutcome::err(format!("Error: path not found: {raw_path}"));
    }

    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("content")
        .to_ascii_lowercase();
    let output = args
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or("lines")
        .to_ascii_lowercase();
    let max_matches = args
        .get("max_matches")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_MATCHES)
        .clamp(1, MAX_MATCHES);

    let filter = match args.get("glob").and_then(Value::as_str) {
        Some(spec) if !spec.trim().is_empty() => match FileFilter::parse(spec) {
            Ok(filter) => filter,
            Err(error) => return ToolOutcome::err(format!("Error: invalid glob: {error}")),
        },
        _ => FileFilter {
            include: None,
            exclude: None,
        },
    };

    if mode == "files" {
        return search_files(
            &ctx.workspace,
            &root,
            &patterns.join("|"),
            &filter,
            max_matches,
        );
    }

    let fixed_strings = args
        .get("fixed_strings")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let word = args.get("word").and_then(Value::as_bool).unwrap_or(false);
    let ignore_case = args
        .get("ignore_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let smart_case = args
        .get("smart_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let joined = patterns
        .iter()
        .map(|pattern| {
            if fixed_strings {
                regex::escape(pattern)
            } else {
                pattern.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    let expression = if word {
        format!(r"\b(?:{joined})\b")
    } else {
        joined
    };
    let case_insensitive = ignore_case
        || (smart_case && !expression.chars().any(|character| character.is_uppercase()));
    let regex = match RegexBuilder::new(&expression)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => return ToolOutcome::err(format!("Error: Invalid regex pattern: {error}")),
    };

    search_content(&ctx.workspace, &root, &regex, &filter, &output, max_matches)
}

fn search_files(
    workspace: &Path,
    root: &Path,
    pattern: &str,
    filter: &FileFilter,
    max_matches: usize,
) -> ToolOutcome {
    let pattern_has_glob = pattern.contains('*') || pattern.contains('?') || pattern.contains('[');
    let glob = if pattern_has_glob {
        match Glob::new(pattern) {
            Ok(glob) => Some(glob.compile_matcher()),
            Err(error) => return ToolOutcome::err(format!("Error: invalid glob: {error}")),
        }
    } else {
        None
    };
    let needle = pattern.to_lowercase();

    let mut files: Vec<String> = Vec::new();
    for entry in walk(root) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let rel = relative_display(workspace, entry.path());
        if !filter.matches(&rel) {
            continue;
        }
        let matched = match &glob {
            Some(glob) => glob.is_match(&rel),
            None => rel.to_lowercase().contains(&needle),
        };
        if matched {
            files.push(rel);
            if files.len() >= max_matches {
                break;
            }
        }
    }
    files.sort();

    if files.is_empty() {
        return ToolOutcome::ok(format!("No files found matching: {pattern}"));
    }
    ToolOutcome::ok(files.join("\n"))
}

fn search_content(
    workspace: &Path,
    root: &Path,
    regex: &Regex,
    filter: &FileFilter,
    output: &str,
    max_matches: usize,
) -> ToolOutcome {
    let mut hits: Vec<(String, usize, String)> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut matched_files: BTreeSet<String> = BTreeSet::new();

    'outer: for entry in walk(root) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let rel = relative_display(workspace, entry.path());
        if !filter.matches(&rel) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut file_count = 0;
        for (index, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                file_count += 1;
                if output == "lines" {
                    hits.push((rel.clone(), index + 1, line.trim_end().to_string()));
                }
                if hits.len() >= max_matches {
                    *counts.entry(rel.clone()).or_insert(0) += file_count;
                    matched_files.insert(rel);
                    break 'outer;
                }
            }
        }
        if file_count > 0 {
            *counts.entry(rel.clone()).or_insert(0) += file_count;
            matched_files.insert(rel);
        }
    }

    if counts.is_empty() {
        return ToolOutcome::ok(format!("No matches found for pattern: {}", regex.as_str()));
    }

    if output == "files" {
        return ToolOutcome::ok(matched_files.into_iter().collect::<Vec<_>>().join("\n"));
    }
    if output == "count" {
        let mut lines = Vec::new();
        for (path, count) in counts {
            lines.push(format!("{path}: {count}"));
        }
        return ToolOutcome::ok(lines.join("\n"));
    }
    let mut lines = Vec::new();
    for (path, line, text) in hits {
        lines.push(format!("{path}:{line}: {text}"));
    }
    ToolOutcome::ok(lines.join("\n"))
}

fn walk(root: &Path) -> impl Iterator<Item = ignore::DirEntry> {
    WalkBuilder::new(root)
        .standard_filters(true)
        .filter_entry(|entry| !skip_entry(entry))
        .build()
        .filter_map(Result::ok)
}

fn skip_entry(entry: &ignore::DirEntry) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    matches!(
        name.as_ref(),
        ".git"
            | "node_modules"
            | "target"
            | "venv"
            | ".venv"
            | ".deep_agents"
            | "dist"
            | "build"
            | "__pycache__"
            | ".next"
            | ".turbo"
    )
}
