use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;
use zip::write::SimpleFileOptions;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

const MAX_PDF_PAGE_CHARS: usize = 3_000;
const MAX_PDF_CHARS: usize = 50_000;

pub async fn read_pdf(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let file_path = args
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if file_path.is_empty() {
        return ToolOutcome::err("Error: file_path is required");
    }
    let path = input_path(ctx, file_path);
    if !path.is_file() {
        return ToolOutcome::err(format!("Error: File not found: {file_path}"));
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
    {
        return ToolOutcome::err(format!("Error: Not a PDF file: {file_path}"));
    }
    let pages = match pdf_extract::extract_text_by_pages(&path) {
        Ok(pages) => pages,
        Err(error) => return ToolOutcome::err(format!("Error reading PDF: {error}")),
    };
    let page_numbers = args
        .get("page_numbers")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let max_pages = args.get("max_pages").and_then(Value::as_u64).unwrap_or(20) as usize;
    let selected: Vec<usize> = if page_numbers.is_empty() {
        let limit = if max_pages == 0 {
            pages.len()
        } else {
            max_pages
        };
        (0..pages.len().min(limit)).collect()
    } else {
        match parse_pages(page_numbers) {
            Ok(pages) => pages,
            Err(error) => return ToolOutcome::err(error),
        }
    };
    let mut output = vec![format!(
        "PDF: {} ({} pages)",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(file_path),
        pages.len()
    )];
    for page in selected {
        let Some(text) = pages.get(page) else {
            break;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        output.push(format!("--- Page {} ---", page + 1));
        output.push(clip_page(text));
    }
    let mut result = output.join("\n");
    if result.len() > MAX_PDF_CHARS {
        let head: String = result.chars().take(MAX_PDF_CHARS / 2).collect();
        let tail: String = result
            .chars()
            .rev()
            .take(MAX_PDF_CHARS / 2)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        result = format!("{head}\n...[large PDF truncated]...\n{tail}");
    }
    if result.trim()
        == format!(
            "PDF: {} ({} pages)",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(file_path),
            pages.len()
        )
    {
        ToolOutcome::ok("(empty PDF)")
    } else {
        ToolOutcome::ok(result)
    }
}

pub async fn create_docx(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let file_path = args
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if file_path.is_empty() {
        return ToolOutcome::err("Error: file_path is required");
    }
    let path = match resolve_workspace_path(&ctx.workspace, file_path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let content = args.get("content").and_then(Value::as_str).unwrap_or("");
    let mut body = String::new();
    if !title.is_empty() {
        body.push_str(&format!(
            "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:rPr><w:b/><w:sz w:val=\"32\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            xml_escape(title)
        ));
    }
    for line in content.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        body.push_str(&format!(
            "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            xml_escape(line)
        ));
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr/></w:body></w:document>"
    );
    let entries = [
        ("[Content_Types].xml", content_types_docx()),
        ("_rels/.rels", root_rels("word/document.xml")),
        ("word/document.xml", document),
    ];
    if let Err(error) = write_zip(&path, &entries) {
        return ToolOutcome::err(format!("Error creating document: {error}"));
    }
    ToolOutcome::ok(format!("Created Word document: {}", path.to_string_lossy()))
}

pub async fn create_xlsx(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let file_path = args
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if file_path.is_empty() {
        return ToolOutcome::err("Error: file_path is required");
    }
    let path = match resolve_workspace_path(&ctx.workspace, file_path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let headers = args
        .get("headers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows = args
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut sheet_rows = Vec::new();
    let mut row_index = 1;
    if !headers.is_empty() {
        sheet_rows.push(sheet_row(row_index, &headers));
        row_index += 1;
    }
    for row in &rows {
        let cells = row.as_array().cloned().unwrap_or_else(|| vec![row.clone()]);
        sheet_rows.push(sheet_row(row_index, &cells));
        row_index += 1;
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{}</sheetData></worksheet>",
        sheet_rows.join("")
    );
    let entries = [
        ("[Content_Types].xml", content_types_xlsx()),
        ("_rels/.rels", root_rels("xl/workbook.xml")),
        ("xl/workbook.xml", workbook_xml()),
        ("xl/_rels/workbook.xml.rels", workbook_rels()),
        ("xl/worksheets/sheet1.xml", document),
    ];
    if let Err(error) = write_zip(&path, &entries) {
        return ToolOutcome::err(format!("Error creating spreadsheet: {error}"));
    }
    ToolOutcome::ok(format!(
        "Created Excel: {} ({} cols, {} rows)",
        path.to_string_lossy(),
        headers.len(),
        rows.len()
    ))
}

fn input_path(ctx: &ToolContext, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.workspace.join(path)
    }
}

fn parse_pages(value: &str) -> Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: usize = start
                .trim()
                .parse()
                .map_err(|_| format!("Error: invalid page range '{part}'"))?;
            let end: usize = end
                .trim()
                .parse()
                .map_err(|_| format!("Error: invalid page range '{part}'"))?;
            if start == 0 || end < start {
                return Err(format!("Error: invalid page range '{part}'"));
            }
            pages.extend(start - 1..end);
        } else {
            let page: usize = part
                .parse()
                .map_err(|_| format!("Error: invalid page '{part}'"))?;
            if page == 0 {
                return Err(format!("Error: invalid page '{part}'"));
            }
            pages.push(page - 1);
        }
    }
    Ok(pages)
}

fn clip_page(text: &str) -> String {
    if text.chars().count() <= MAX_PDF_PAGE_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_PDF_PAGE_CHARS / 2).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(MAX_PDF_PAGE_CHARS / 2)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}\n...[truncated]...\n{tail}")
}

fn write_zip(path: &Path, entries: &[(&str, String)]) -> Result<(), String> {
    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    for (name, content) in entries {
        zip.start_file(*name, SimpleFileOptions::default())
            .map_err(|error| error.to_string())?;
        zip.write_all(content.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    zip.finish().map_err(|error| error.to_string())?;
    Ok(())
}

fn sheet_row(row: usize, cells: &[Value]) -> String {
    let mut output = format!("<row r=\"{row}\">");
    for (index, value) in cells.iter().enumerate() {
        let cell = format!("{}{}", column(index), row);
        match value {
            Value::Number(number) => {
                output.push_str(&format!("<c r=\"{cell}\"><v>{number}</v></c>"));
            }
            Value::Bool(value) => {
                output.push_str(&format!(
                    "<c r=\"{cell}\" t=\"b\"><v>{}</v></c>",
                    if *value { 1 } else { 0 }
                ));
            }
            _ => {
                output.push_str(&format!(
                    "<c r=\"{cell}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    xml_escape(value.as_str().unwrap_or(""))
                ));
            }
        }
    }
    output.push_str("</row>");
    output
}

fn column(mut index: usize) -> String {
    let mut name = String::new();
    index += 1;
    while index > 0 {
        let remainder = (index - 1) % 26;
        name.insert(0, (b'A' + remainder as u8) as char);
        index = (index - 1) / 26;
    }
    name
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn root_rels(target: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"{target}\"/></Relationships>"
    )
}

fn content_types_docx() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>".to_string()
}

fn content_types_xlsx() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>".to_string()
}

fn workbook_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>".to_string()
}

fn workbook_rels() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".to_string()
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::sync::Arc;

    use crate::QuestionBroker;

    use super::*;

    #[tokio::test]
    async fn creates_docx_and_xlsx_archives() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "docs", Arc::new(QuestionBroker::new()));
        let docx = create_docx(
            &ctx,
            &serde_json::json!({
                "file_path": "out/doc.docx",
                "title": "Report",
                "content": "one\ntwo"
            }),
        )
        .await;
        assert!(!docx.is_error, "{}", docx.content);
        assert!(dir.path().join("out/doc.docx").is_file());

        let xlsx = create_xlsx(
            &ctx,
            &serde_json::json!({
                "file_path": "out/data.xlsx",
                "headers": ["name", "count"],
                "rows": [["alpha", 2], ["beta", 3]]
            }),
        )
        .await;
        assert!(!xlsx.is_error, "{}", xlsx.content);
        let file = File::open(dir.path().join("out/data.xlsx")).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(archive.by_name("xl/worksheets/sheet1.xml").is_ok());
        let mut sheet = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        assert!(sheet.contains("alpha"));
        assert!(sheet.contains("<v>2</v>"));
    }

    #[tokio::test]
    async fn missing_pdf_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "docs", Arc::new(QuestionBroker::new()));
        let result = read_pdf(&ctx, &serde_json::json!({"file_path": "missing.pdf"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("File not found"));
    }

    #[test]
    fn parses_page_ranges() {
        assert_eq!(parse_pages("1-3,5").unwrap(), vec![0, 1, 2, 4]);
        assert!(parse_pages("0-2").is_err());
    }
}
