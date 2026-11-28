//! Code Mode catalog: OpenCode v2 `packages/core/src/codemode/catalog.ts` and
//! `packages/core/src/codemode/instructions.ts`, ported to Rust.
//!
//! The selection algorithm (namespace grouping, pinned-first round-robin,
//! inline budget) and the `# Code Mode` prompt template are byte-identical to
//! OpenCode. Only the tool inventory source differs (Stark dispatch names).
//!
//! MIT attribution: algorithm and prompt © OpenCode authors (MIT).

/// One tool as the worker reports it: dotted path, description, signature.
#[derive(Debug, Clone)]
pub struct CatalogItem {
    pub path: String,
    pub description: String,
    pub signature: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Listing {
    pub path: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NamespaceSummary {
    pub name: String,
    pub description: Option<String>,
    /// Every tool path in this namespace, whether shown or not.
    pub tools: Vec<String>,
    pub entries: Vec<Listing>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    pub total: usize,
    pub namespaces: Vec<NamespaceSummary>,
}

const DESCRIPTION_LIMIT: usize = 2_000;
const CHARACTERS_PER_TOKEN: usize = 4;
/// Inline budget: OpenCode `INLINE_BUDGET = 2_000` (tokens).
pub const INLINE_BUDGET_TOKENS: usize = 2_000;

/// Exact `search` signature from `@opencode/codemode` (`searchSignature`).
pub const SEARCH_SIGNATURE: &str = "search(query: string | {\n  query?: string,\n  namespace?: string,\n  /** @integer @exclusiveMinimum 0 */\n  limit?: number,\n  /** @integer @minimum 0 */\n  offset?: number,\n}): {\n  items: Array<{\n      path: string,\n      description: string,\n      signature: string,\n    }>,\n  /** @integer @minimum 0 */\n  remaining: number,\n  next: {\n      /** @integer @minimum 0 */\n      offset: number,\n    } | null,\n}";

/// OpenCode compares strings with JavaScript `<`, which orders UTF-16 code
/// units. For BMP text this equals code-point order; astral characters sort
/// before high BMP characters in JS, unlike Rust's UTF-8 byte order.
fn js_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

pub fn cost(text: &str) -> usize {
    // OpenCode `Math.round(text.length / 4)`, where `.length` is UTF-16 code
    // units. Round, never ceil: five units cost one token, six cost two.
    let units = text.encode_utf16().count();
    (units + CHARACTERS_PER_TOKEN / 2) / CHARACTERS_PER_TOKEN
}

fn truncate_to(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_string();
    }
    // JS `slice` counts UTF-16 units. A lone surrogate cannot exist in Rust,
    // so stop at the last whole scalar that fits.
    let mut units = 0usize;
    let mut out = String::new();
    for character in text.chars() {
        let width = character.len_utf16();
        if units + width > max {
            break;
        }
        units += width;
        out.push(character);
    }
    out
}

/// OpenCode `listing` for one prepared catalog item.
pub fn listing_for_item(item: &CatalogItem) -> Listing {
    listing_for(&item.path, &item.signature, &item.description, item.pinned)
}

fn listing_for(path: &str, signature: &str, description: &str, pinned: bool) -> Listing {
    if pinned {
        let mut lines = description.trim().split('\n');
        let first = lines.next().unwrap_or("").trim_end().to_string();
        let suffix = if first.is_empty() {
            String::new()
        } else {
            format!(" // {first}")
        };
        let mut out = format!("  - {signature}{suffix}");
        for rest in lines {
            out.push('\n');
            out.push_str(format!("    // {}", rest.trim_end()).trim_end());
        }
        return Listing {
            path: path.to_string(),
            line: out,
        };
    }
    let first_line: String = description
        .split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let short = if first_line.encode_utf16().count() > DESCRIPTION_LIMIT {
        format!(
            "{}...",
            truncate_to(&first_line, DESCRIPTION_LIMIT.saturating_sub(3))
        )
    } else {
        first_line
    };
    let suffix = if short.is_empty() {
        String::new()
    } else {
        format!(" // {short}")
    };
    Listing {
        path: path.to_string(),
        line: format!("  - {signature}{suffix}"),
    }
}

/// Port of `CodeModeCatalog.summarize`: group by first path segment, keep
/// every namespace visible, then select full listings one per namespace per
/// round in path order until the inline budget is exhausted. Pinned tools are
/// always shown.
pub fn summarize(items: &[CatalogItem], budget_tokens: usize) -> Summary {
    // Flatten is trivial here: items already carry full dotted paths.
    let mut by_namespace: std::collections::BTreeMap<String, Vec<&CatalogItem>> =
        std::collections::BTreeMap::new();
    let mut descriptions: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();
    for item in items {
        let namespace = item
            .path
            .split('.')
            .next()
            .unwrap_or(&item.path)
            .to_string();
        by_namespace
            .entry(namespace.clone())
            .or_default()
            .push(item);
        descriptions.entry(namespace).or_insert(None);
    }
    struct NamespaceWork {
        name: String,
        description: Option<String>,
        tools: Vec<String>,
        selection_order: Vec<Listing>,
        selected: Vec<Listing>,
        index: usize,
    }
    let mut namespaces: Vec<NamespaceWork> = Vec::new();
    for (name, mut entries) in by_namespace {
        entries.sort_by(|a, b| js_cmp(&a.path, &b.path));
        let description = descriptions.remove(&name).flatten();
        let tools: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
        let mut selected: Vec<Listing> = Vec::new();
        let mut selection_order: Vec<Listing> = Vec::new();
        for entry in &entries {
            let listing = listing_for(
                &entry.path,
                &entry.signature,
                &entry.description,
                entry.pinned,
            );
            if entry.pinned {
                selected.push(listing);
            } else {
                selection_order.push(listing);
            }
        }
        namespaces.push(NamespaceWork {
            name,
            description,
            tools,
            selection_order,
            selected,
            index: 0,
        });
    }
    namespaces.sort_by(|a, b| js_cmp(&a.name, &b.name));
    let mut remaining: isize = budget_tokens as isize
        - namespaces
            .iter()
            .map(|namespace| {
                cost(&namespace_line(
                    &namespace.name,
                    namespace.description.as_deref(),
                    &namespace.tools,
                    0,
                )) as isize
            })
            .sum::<isize>()
        - namespaces
            .iter()
            .flat_map(|namespace| namespace.selected.iter())
            .map(|listing| cost(&listing.line) as isize)
            .sum::<isize>();
    // Round-robin: one listing per namespace per round, in path order, until
    // the budget is exhausted (OpenCode `summarize`).
    let mut active: Vec<usize> = (0..namespaces.len()).collect();
    while !active.is_empty() {
        let mut next_active = Vec::new();
        for index in active {
            let namespace = &mut namespaces[index];
            let Some(entry) = namespace.selection_order.get(namespace.index).cloned() else {
                continue;
            };
            if cost(&entry.line) as isize > remaining {
                continue;
            }
            remaining -= cost(&entry.line) as isize;
            namespace.index += 1;
            namespace.selected.push(entry);
            if namespace.index < namespace.selection_order.len() {
                next_active.push(index);
            }
        }
        // Namespaces that cannot afford their next listing drop out; the rest
        // carry over. Indices only advance, so the loop always terminates.
        active = next_active;
    }
    Summary {
        total: items.len(),
        namespaces: namespaces
            .into_iter()
            .map(|namespace| NamespaceSummary {
                name: namespace.name,
                description: namespace.description,
                tools: namespace.tools,
                entries: namespace.selected,
            })
            .collect(),
    }
}

pub fn namespace_line(
    name: &str,
    description: Option<&str>,
    tools: &[String],
    shown: usize,
) -> String {
    let shown = if shown == usize::MAX {
        tools.len()
    } else {
        shown
    };
    let count = if tools.len() == 1 {
        "1 tool".to_string()
    } else {
        format!("{} tools", tools.len())
    };
    let label = if shown == tools.len() {
        count
    } else if shown == 0 {
        format!("{count}, none shown")
    } else {
        format!("{count}, {shown} shown")
    };
    match description {
        Some(description) => {
            format!("- {name} ({label}) // {description}")
        }
        _ => format!("- {name} ({label})"),
    }
}

fn prompt_has_more_tools(summary: &Summary) -> bool {
    !complete(summary)
}

fn complete(summary: &Summary) -> bool {
    summary
        .namespaces
        .iter()
        .all(|namespace| namespace.entries.len() == namespace.tools.len())
}

/// Port of `CodeModeInstructions.render`: the exact `# Code Mode` prompt.
pub fn render(summary: &Summary) -> String {
    if summary.total == 0 {
        return "No Code Mode tools are currently available. Later Code Mode catalog updates may add or remove tools. Do not call `execute` unless there is at least one available Code Mode tool.".to_string();
    }
    let mut tool_lines: Vec<String> = Vec::new();
    for namespace in &summary.namespaces {
        tool_lines.push(namespace_line(
            &namespace.name,
            namespace.description.as_deref(),
            &namespace.tools,
            namespace.entries.len(),
        ));
        for entry in &namespace.entries {
            tool_lines.push(entry.line.clone());
        }
    }
    let head = if prompt_has_more_tools(summary) {
        format!(
            "# Code Mode\n\nThe tools listed below, and `search`, cannot be called directly: they only work as JavaScript inside the `execute` tool. Every other tool you have is a normal tool call: call it directly, never inside `execute`.\n\nThe catalog is partial. Before calling a tool that is not listed below, find it with the global `search(...)` function. Do not guess tool names. Each result's `path` is the complete call expression: write it out as code, for example `await tools.ns.tool_name({{ ... }})`. Do not use it as a key such as `tools[path]`.\n\n- {SEARCH_SIGNATURE}"
        )
    } else {
        "# Code Mode\n\nThe tools listed below cannot be called directly: they only work as JavaScript inside the `execute` tool. Every other tool you have is a normal tool call: call it directly, never inside `execute`.\n\nThe catalog is complete. Do not guess tool names.".to_string()
    };
    format!(
        "{head}\n\n## Code Mode tools (call only inside `execute`)\n\n{}",
        tool_lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> Vec<CatalogItem> {
        vec![
            CatalogItem {
                path: "read".to_string(),
                description: "Read a file.\nSecond line.\nThird line.".to_string(),
                signature: "tools.read({\n  file_path: string,\n}): Promise<string | null>"
                    .to_string(),
                pinned: true,
            },
            CatalogItem {
                path: "search".to_string(),
                description: "Search ünïcode ✨ and a very long first line that should be clipped only when it exceeds the two thousand character description limit; this one is short enough.".to_string(),
                signature: "tools.search({ query: string }): Promise<string | null>".to_string(),
                pinned: false,
            },
            CatalogItem {
                path: "github.create_issue".to_string(),
                description: "Create an issue.".to_string(),
                signature: "tools.github.create_issue({\n  title: string,\n}): Promise<string | null>"
                    .to_string(),
                pinned: false,
            },
            CatalogItem {
                path: "github.get_issue".to_string(),
                description: "Get an issue.".to_string(),
                signature: "tools.github.get_issue({\n  id: number,\n}): Promise<string | null>"
                    .to_string(),
                pinned: false,
            },
            CatalogItem {
                path: "github.list_issues".to_string(),
                description: "List issues.".to_string(),
                signature: "tools.github.list_issues({}): Promise<string | null>".to_string(),
                pinned: false,
            },
        ]
    }

    /// Git may materialize text fixtures with CRLF on Windows. The catalog
    /// sent to providers is always LF, so normalize the expected side.
    fn fixture(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    #[test]
    fn catalog_prompt_matches_opencode_source() {
        let full = render(&summarize(&inventory(), 2_000));
        assert_eq!(full, fixture(include_str!("testdata/opencode_catalog.txt")));

        let partial = render(&summarize(&inventory(), 30));
        assert_eq!(
            partial,
            fixture(include_str!("testdata/opencode_catalog_partial.txt"))
        );
    }

    #[test]
    fn js_cost_uses_utf16_code_units_and_rounds() {
        // OpenCode `Math.round(length / 4)`: five units round down to one.
        assert_eq!(cost(""), 0);
        assert_eq!(cost("abcd"), 1);
        assert_eq!(cost("abcde"), 1);
        assert_eq!(cost("abcdef"), 2);
        assert_eq!(cost("😀"), 1);
        assert_eq!(cost("😀😀"), 1);
        assert_eq!(cost("ü✨"), 1);
        assert_eq!(cost("a😀"), 1);
        assert_eq!(truncate_to("😀😀", 2), "😀");
        assert_eq!(truncate_to("a😀", 2), "a");
        // JS orders UTF-16 units: an astral character (high surrogate D83D)
        // sorts before U+FFFD, though its code point is larger.
        assert_eq!(js_cmp("😀", "�"), std::cmp::Ordering::Less);
    }
}
