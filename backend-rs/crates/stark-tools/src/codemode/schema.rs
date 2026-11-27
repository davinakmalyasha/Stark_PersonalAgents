//! JSON Schema → TypeScript signature renderer: OpenCode v2
//! `packages/codemode/src/codemode/tool-schema.ts`, ported to Rust.
//!
//! Code Mode never validates values against these schemas (the host already
//! did); they only shape the model-visible signature. The rendering is
//! byte-identical to OpenCode so catalog bytes match the worker's runtime.
//!
//! MIT attribution: rendering algorithm © OpenCode authors (MIT).

use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn is_js_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' || first == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn render_key(name: &str) -> String {
    if is_js_identifier(name) {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap_or_else(|_| "\"?\"".to_string())
    }
}

fn render_literal(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
}

fn definition_name(reference: &str) -> Option<String> {
    // JsonPointer.parseUriFragment: "#/$defs/Name" or "#/definitions/Name".
    let fragment = reference.strip_prefix('#')?;
    let mut parts: Vec<String> = Vec::new();
    for token in fragment.split('/') {
        if token.is_empty() {
            continue;
        }
        parts.push(token.replace("~1", "/").replace("~0", "~"));
    }
    if parts.len() == 2 && (parts[0] == "$defs" || parts[0] == "definitions") {
        Some(parts[1].clone())
    } else {
        None
    }
}

fn effect_number_sentinel(schema: &Value) -> bool {
    if schema.get("type").and_then(Value::as_str) != Some("string") {
        return false;
    }
    match schema.get("enum").and_then(Value::as_array) {
        Some(values) if !values.is_empty() => values.iter().all(|value| {
            matches!(
                value.as_str(),
                Some("NaN") | Some("Infinity") | Some("-Infinity")
            )
        }),
        _ => false,
    }
}

fn is_effect_empty_struct(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    let Some(not) = object.get("not").and_then(Value::as_object) else {
        return false;
    };
    if not.get("type").and_then(Value::as_str) != Some("null") || not.len() != 1 {
        return false;
    }
    const ANNOTATIONS: &[&str] = &[
        "title",
        "description",
        "default",
        "examples",
        "readOnly",
        "writeOnly",
    ];
    object
        .keys()
        .all(|key| key == "not" || ANNOTATIONS.contains(&key.as_str()))
}

fn intersection(members: &[String]) -> String {
    let concrete: Vec<&String> = members
        .iter()
        .filter(|member| member.as_str() != "unknown")
        .collect();
    if concrete.is_empty() {
        return "unknown".to_string();
    }
    if concrete.len() == 1 {
        return concrete[0].clone();
    }
    concrete
        .iter()
        .map(|member| {
            if member.contains(" | ") {
                format!("({member})")
            } else {
                member.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" & ")
}

const MAX_RENDER_DEPTH: usize = 8;

#[derive(Debug, Clone, Default)]
struct RenderContext {
    definitions: HashMap<String, Value>,
    pretty: bool,
}

fn doc_tags(schema: &Value) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    if schema.get("deprecated").and_then(Value::as_bool) == Some(true) {
        tags.push("@deprecated".to_string());
    }
    if let Some(default) = schema.get("default")
        && let Ok(rendered) = serde_json::to_string(default)
    {
        tags.push(format!("@default {rendered}"));
    }
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        tags.push(format!("@format {format}"));
    }
    if schema.get("type").and_then(Value::as_str) == Some("integer") {
        tags.push("@integer".to_string());
    }
    let integer = schema.get("type").and_then(Value::as_str) == Some("integer");
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && !(integer && minimum == i64::MIN as f64)
    {
        let text = schema
            .get("minimum")
            .map(|value| value.to_string())
            .unwrap_or_default();
        tags.push(format!("@minimum {text}"));
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && !(integer && maximum == i64::MAX as f64)
    {
        let text = schema
            .get("maximum")
            .map(|value| value.to_string())
            .unwrap_or_default();
        tags.push(format!("@maximum {text}"));
    }
    if let Some(value) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
        let _ = value;
        let text = schema
            .get("exclusiveMinimum")
            .map(|value| value.to_string())
            .unwrap_or_default();
        tags.push(format!("@exclusiveMinimum {text}"));
    }
    if let Some(value) = schema.get("exclusiveMaximum").and_then(Value::as_f64) {
        let _ = value;
        let text = schema
            .get("exclusiveMaximum")
            .map(|value| value.to_string())
            .unwrap_or_default();
        tags.push(format!("@exclusiveMaximum {text}"));
    }
    if let Some(value) = schema.get("multipleOf") {
        tags.push(format!("@multipleOf {value}"));
    }
    if let Some(value) = schema.get("minLength") {
        tags.push(format!("@minLength {value}"));
    }
    if let Some(value) = schema.get("maxLength") {
        tags.push(format!("@maxLength {value}"));
    }
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        tags.push(format!("@pattern {pattern}"));
    }
    if let Some(value) = schema.get("minItems") {
        tags.push(format!("@minItems {value}"));
    }
    if let Some(value) = schema.get("maxItems") {
        tags.push(format!("@maxItems {value}"));
    }
    if schema.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        tags.push("@uniqueItems true".to_string());
    }
    tags
}

fn doc_lines(schema: &Value, width: usize) -> Vec<String> {
    let summary = doc_tags(schema).join(" ");
    let description = schema
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut lines: Vec<String> = description
        .split('\n')
        .map(|line| line.trim_end().to_string())
        .collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let inline = if lines.len() == 1 {
        let first = &lines[0];
        let dotted = if first.ends_with('.') {
            first.clone()
        } else {
            format!("{first}.")
        };
        format!("{dotted} {summary}")
    } else {
        summary.clone()
    };
    if !summary.is_empty()
        && lines.len() == 1
        && !summary.contains('\n')
        && width + inline.len() + 7 <= 120
    {
        vec![inline]
    } else {
        let mut out = lines;
        if !summary.is_empty() {
            out.extend(summary.split('\n').map(str::to_string));
        }
        out
    }
}

/// Neutralize `*/` so schema text cannot terminate generated documentation.
fn jsdoc(schema: &Value, pad: &str) -> String {
    let content = doc_lines(schema, pad.len());
    let types: Vec<String> = match schema.get("type") {
        Some(Value::String(single)) => vec![single.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let mut content = content;
    let mut append = |label: &str, child: &Value| {
        for (index, line) in doc_lines(child, pad.len() + label.len() + 2)
            .iter()
            .enumerate()
        {
            if index == 0 {
                content.push(format!("{label}: {line}"));
            } else if line.is_empty() {
                content.push(String::new());
            } else {
                content.push(format!("  {line}"));
            }
        }
    };
    if types.iter().any(|kind| kind == "array")
        && let Some(items) = schema.get("items")
    {
        append("Each item", items);
    }
    if (types.iter().any(|kind| kind == "object") || schema.get("properties").is_some())
        && let Some(Value::Object(_)) = schema.get("additionalProperties")
    {
        let additional = schema.get("additionalProperties").unwrap();
        let label = if schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| !properties.is_empty())
        {
            "Each additional value"
        } else {
            "Each value"
        };
        append(label, additional);
    }
    if content.is_empty() {
        return String::new();
    }
    let escaped: Vec<String> = content
        .iter()
        .map(|line| line.replace("*/", "* /"))
        .collect();
    if escaped.len() == 1 && pad.len() + escaped[0].len() + 7 <= 120 {
        return format!("{pad}/** {} */\n", escaped[0]);
    }
    let body = escaped
        .iter()
        .map(|line| {
            if line.is_empty() {
                format!("{pad} *")
            } else {
                format!("{pad} * {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{pad}/**\n{body}\n{pad} */\n")
}

fn schema_without(schema: &Value, drop: &[&str]) -> Value {
    match schema {
        Value::Object(object) => {
            let mut next = object.clone();
            for key in drop {
                next.remove(*key);
            }
            Value::Object(next)
        }
        other => other.clone(),
    }
}

fn render_schema(
    schema: &Value,
    ctx: &RenderContext,
    depth: usize,
    seen: &HashSet<String>,
) -> String {
    if depth > MAX_RENDER_DEPTH {
        return "unknown".to_string();
    }
    let mut nested = ctx.clone();
    let has_local = schema.get("definitions").is_some() || schema.get("$defs").is_some();
    if has_local {
        if let Some(definitions) = schema.get("definitions").and_then(Value::as_object) {
            for (key, value) in definitions {
                nested.definitions.insert(key.clone(), value.clone());
            }
        }
        if let Some(defs) = schema.get("$defs").and_then(Value::as_object) {
            for (key, value) in defs {
                nested.definitions.insert(key.clone(), value.clone());
            }
        }
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = definition_name(reference);
        let target = name.as_ref().and_then(|name| nested.definitions.get(name));
        match (name, target) {
            (Some(name), Some(target)) if !seen.contains(&name) => {
                let mut next_seen = seen.clone();
                next_seen.insert(name.clone());
                let target = target.clone();
                let left = render_schema(&target, &nested, depth, &next_seen);
                let right =
                    render_schema(&schema_without(schema, &["$ref"]), &nested, depth + 1, seen);
                return intersection(&[left, right]);
            }
            _ => return "unknown".to_string(),
        }
    }
    if schema.get("const").is_some_and(|value| !value.is_null())
        || schema.as_object().is_some_and(|o| o.contains_key("const"))
    {
        return render_literal(&schema["const"]);
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values
            .iter()
            .map(render_literal)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    let listed = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
        .cloned();
    if let Some(listed) = listed {
        let has_number = listed
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("number"));
        let alternatives: Vec<Value> = if has_number {
            listed
                .into_iter()
                .filter(|item| !effect_number_sentinel(item))
                .collect()
        } else {
            listed
        };
        if alternatives
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("number"))
            && alternatives.iter().all(|item| {
                let kind = item.get("type").and_then(Value::as_str);
                kind == Some("number") || effect_number_sentinel(item)
            })
        {
            return "number".to_string();
        }
        if alternatives.len() == 2
            && alternatives[0].get("type").and_then(Value::as_str) == Some("object")
            && alternatives[0].get("properties").is_none()
            && alternatives[1].get("type").and_then(Value::as_str) == Some("array")
            && alternatives[1].get("items").is_none()
        {
            return "{}".to_string();
        }
        let members: Vec<String> = alternatives
            .iter()
            .map(|item| render_schema(item, &nested, depth + 1, seen))
            .collect();
        if members.iter().any(|member| member == "unknown") {
            return "unknown".to_string();
        }
        let rest = if schema.get("anyOf").is_some() {
            schema_without(schema, &["anyOf"])
        } else {
            schema_without(schema, &["oneOf"])
        };
        let mut parts = vec![members.join(" | ")];
        parts.push(render_schema(&rest, &nested, depth + 1, seen));
        return intersection(&parts);
    }
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        if all_of.iter().any(|item| {
            has_unresolved_ref(item, &nested.definitions, &HashSet::new(), &mut Vec::new())
        }) {
            return "unknown".to_string();
        }
        let mut parts = vec![render_schema(
            &schema_without(schema, &["allOf"]),
            &nested,
            depth + 1,
            seen,
        )];
        for item in all_of {
            parts.push(render_schema(item, &nested, depth + 1, seen));
        }
        return intersection(&parts);
    }
    if let Some(types) = schema.get("type").and_then(Value::as_array) {
        return types
            .iter()
            .map(|kind| {
                let mut single = schema.clone();
                if let Value::Object(object) = &mut single {
                    object.insert("type".to_string(), kind.clone());
                }
                render_schema(&single, &nested, depth + 1, seen)
            })
            .collect::<Vec<_>>()
            .join(" | ");
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => return "string".to_string(),
        Some("number") | Some("integer") => return "number".to_string(),
        Some("boolean") => return "boolean".to_string(),
        Some("null") => return "null".to_string(),
        Some("array") => {
            let items = schema.get("items").cloned().unwrap_or(json!({}));
            return format!("Array<{}>", render_schema(&items, &nested, depth + 1, seen));
        }
        _ => {}
    }
    let is_object = schema.get("type").and_then(Value::as_str) == Some("object")
        || schema.get("properties").is_some();
    if is_object {
        let required: HashSet<String> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let properties: Vec<(String, Value)> = schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let index_type = match schema.get("additionalProperties") {
            Some(Value::Object(_)) => Some(render_schema(
                &schema["additionalProperties"],
                &nested,
                depth + 1,
                seen,
            )),
            _ => None,
        };
        let field = |(name, value): &(String, Value)| {
            let optional = if required.contains(name) { "" } else { "?" };
            format!(
                "{}{optional}: {}",
                render_key(name),
                render_schema(value, &nested, depth + 1, seen)
            )
        };
        if !nested.pretty {
            let mut fields: Vec<String> = properties.iter().map(field).collect();
            if let Some(index_type) = index_type {
                fields.push(format!("[key: string]: {index_type}"));
            }
            if fields.is_empty() {
                return "{}".to_string();
            }
            return format!("{{ {} }}", fields.join("; "));
        }
        if properties.is_empty() && index_type.is_none() {
            return "{}".to_string();
        }
        let pad = "  ".repeat(depth + 1);
        let mut lines: Vec<String> = properties
            .iter()
            .map(|entry| format!("{}{},", jsdoc(&entry.1, &pad), pad.clone() + &field(entry)))
            .collect();
        if let Some(index_type) = index_type {
            lines.push(format!("{pad}[key: string]: {index_type},"));
        }
        return format!("{{\n{}\n{}}}", lines.join("\n"), "  ".repeat(depth));
    }
    if is_effect_empty_struct(schema) {
        return "{}".to_string();
    }
    "unknown".to_string()
}

fn has_unresolved_ref(
    schema: &Value,
    definitions: &HashMap<String, Value>,
    seen: &HashSet<String>,
    visited: &mut Vec<*const Value>,
) -> bool {
    let pointer = schema as *const Value;
    if visited.contains(&pointer) {
        return false;
    }
    visited.push(pointer);
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        match definition_name(reference) {
            Some(name) => match definitions.get(&name) {
                Some(target) if !seen.contains(&name) => {
                    let mut next_seen = seen.clone();
                    next_seen.insert(name);
                    if has_unresolved_ref(target, definitions, &next_seen, visited) {
                        return true;
                    }
                }
                _ => return true,
            },
            None => return true,
        }
    }
    let mut children: Vec<&Value> = Vec::new();
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(items) = schema.get(key).and_then(Value::as_array) {
            children.extend(items.iter());
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        children.extend(properties.values());
    }
    if let Some(items) = schema.get("items") {
        children.push(items);
    }
    if let Some(Value::Object(_)) = schema.get("additionalProperties") {
        children.push(&schema["additionalProperties"]);
    }
    children
        .iter()
        .any(|child| has_unresolved_ref(child, definitions, seen, visited))
}

/// Render a raw JSON Schema document to TypeScript (OpenCode `jsonSchemaToTypeScript`).
pub fn json_schema_to_typescript(schema: &Value, pretty: bool) -> String {
    let ctx = RenderContext {
        definitions: HashMap::new(),
        pretty,
    };
    render_schema(schema, &ctx, 0, &HashSet::new())
}

/// `tools.ns.tool_name` / `tools["odd-name"]` (OpenCode `toolExpression`, with
/// the `tools` prefix included).
pub fn tool_expression(path: &str) -> String {
    let mut out = String::from("tools");
    for segment in path.split('.') {
        if is_js_identifier(segment) {
            out.push('.');
            out.push_str(segment);
        } else {
            out.push_str(&format!(
                "[{}]",
                serde_json::to_string(segment).unwrap_or_default()
            ));
        }
    }
    out
}

/// OpenCode `stringParameter`: a freeform tool with one required string field
/// renders and accepts that string directly.
fn string_parameter(input_schema: &Value) -> Option<(String, String)> {
    let properties = input_schema.get("properties").and_then(Value::as_object)?;
    if properties.len() != 1 {
        return None;
    }
    let (name, field) = properties.iter().next()?;
    if field.get("type").and_then(Value::as_str) != Some("string") {
        return None;
    }
    let required = input_schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.as_str() == Some(name.as_str()))
        });
    if !required {
        return None;
    }
    let label = if is_js_identifier(name) {
        name.as_str()
    } else {
        "input"
    };
    let mut doc = jsdoc(field, "");
    if doc.ends_with('\n') {
        doc.pop();
        doc.push(' ');
    }
    let kind = json_schema_to_typescript(field, false);
    Some((label.to_string(), format!("({doc}{label}: {kind})")))
}

/// OpenCode `isEmptyInput` for raw JSON Schema: the rendered input is `{}`.
pub fn is_empty_input(input_schema: &Value) -> bool {
    json_schema_to_typescript(input_schema, false) == "{}"
}

/// OpenCode `describeTool` signature:
/// `{expression}({input}): Promise<{output}>`.
pub fn tool_signature(
    path: &str,
    input_schema: &Value,
    output_schema: &Value,
    freeform: bool,
) -> String {
    let params = if freeform {
        string_parameter(input_schema)
            .map(|(_, signature)| signature)
            .unwrap_or_else(|| {
                if is_empty_input(input_schema) {
                    "()".to_string()
                } else {
                    format!("({})", json_schema_to_typescript(input_schema, true))
                }
            })
    } else if is_empty_input(input_schema) {
        "()".to_string()
    } else {
        format!("({})", json_schema_to_typescript(input_schema, true))
    };
    format!(
        "{}{}{}",
        tool_expression(path),
        params,
        format!(
            ": Promise<{}>",
            json_schema_to_typescript(output_schema, true)
        )
    )
}

use serde_json::json;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_read_like_opencode() {
        let input = json!({
            "type": "object",
            "properties": {"file_path": {"type": "string"}},
            "required": ["file_path"],
        });
        let output = json!({"type": ["string", "null"]});
        assert_eq!(
            tool_signature("read", &input, &output, false),
            "tools.read({\n  file_path: string,\n}): Promise<string | null>"
        );
    }

    #[test]
    fn search_signature_literal_is_used_verbatim() {
        // Captured from `@opencode/codemode@2.4.1` (`searchSignature`); the
        // prompt embeds it verbatim instead of re-deriving it from Effect
        // schemas that Rust cannot construct.
        assert!(super::super::catalog::SEARCH_SIGNATURE.starts_with("search(query: string | {"));
        assert!(super::super::catalog::SEARCH_SIGNATURE.contains("remaining: number,"));
    }

    #[test]
    fn renders_integer_tags_like_opencode() {
        let input = json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "exclusiveMinimum": 0},
                "offset": {"type": "integer", "minimum": 0},
            },
        });
        let rendered = json_schema_to_typescript(&input, true);
        assert!(
            rendered.contains("/** @integer @exclusiveMinimum 0 */"),
            "{rendered}"
        );
        assert!(
            rendered.contains("/** @integer @minimum 0 */"),
            "{rendered}"
        );
    }
}
