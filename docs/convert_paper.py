"""
convert_paper.py — Convert paper_stark.md to formatted paper_stark.docx
Format: A4, Times New Roman 12pt, 1.5 line spacing, proper headings.

Usage:
    python convert_paper.py
    Output: D:\\MyProject\\LangChain\\docs\\paper_stark.docx
"""
import re
from pathlib import Path
from docx import Document
from docx.shared import Pt, Cm, Inches, RGBColor
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.enum.section import WD_ORIENT
from docx.oxml.ns import qn

# ── Config ──────────────────────────────────────────────────────────────
INPUT_MD = Path(__file__).parent / "paper_stark.md"
OUTPUT_DOCX = Path(__file__).parent / "paper_stark.docx"

FONT_NAME = "Times New Roman"
FONT_SIZE_BODY = Pt(12)
FONT_SIZE_H1 = Pt(16)
FONT_SIZE_H2 = Pt(14)
FONT_SIZE_H3 = Pt(12)
LINE_SPACING = 1.5
MARGIN_TOP = Cm(2.54)
MARGIN_BOTTOM = Cm(2.54)
MARGIN_LEFT = Cm(3)
MARGIN_RIGHT = Cm(2)


def setup_document():
    """Create document with proper formatting."""
    doc = Document()

    # Page setup
    for section in doc.sections:
        section.top_margin = MARGIN_TOP
        section.bottom_margin = MARGIN_BOTTOM
        section.left_margin = MARGIN_LEFT
        section.right_margin = MARGIN_RIGHT
        section.page_width = Cm(21)
        section.page_height = Cm(29.7)

    # Default font
    style = doc.styles['Normal']
    font = style.font
    font.name = FONT_NAME
    font.size = FONT_SIZE_BODY
    pf = style.paragraph_format
    pf.line_spacing = LINE_SPACING
    pf.space_after = Pt(6)
    pf.space_before = Pt(0)

    # Heading 1
    h1 = doc.styles['Heading 1']
    h1.font.name = FONT_NAME
    h1.font.size = FONT_SIZE_H1
    h1.font.bold = True
    h1.font.color.rgb = RGBColor(0, 0, 0)
    h1.paragraph_format.line_spacing = LINE_SPACING
    h1.paragraph_format.space_before = Pt(24)
    h1.paragraph_format.space_after = Pt(12)

    # Heading 2
    h2 = doc.styles['Heading 2']
    h2.font.name = FONT_NAME
    h2.font.size = FONT_SIZE_H2
    h2.font.bold = True
    h2.font.color.rgb = RGBColor(0, 0, 0)
    h2.paragraph_format.line_spacing = LINE_SPACING
    h2.paragraph_format.space_before = Pt(18)
    h2.paragraph_format.space_after = Pt(8)

    # Heading 3
    h3 = doc.styles['Heading 3']
    h3.font.name = FONT_NAME
    h3.font.size = FONT_SIZE_H3
    h3.font.bold = True
    h3.font.color.rgb = RGBColor(0, 0, 0)
    h3.paragraph_format.line_spacing = LINE_SPACING
    h3.paragraph_format.space_before = Pt(12)
    h3.paragraph_format.space_after = Pt(6)

    # Heading 4
    h4 = doc.styles['Heading 4']
    h4.font.name = FONT_NAME
    h4.font.size = Pt(12)
    h4.font.bold = True
    h4.font.italic = True
    h4.font.color.rgb = RGBColor(0, 0, 0)
    h4.paragraph_format.line_spacing = LINE_SPACING
    h4.paragraph_format.space_before = Pt(12)
    h4.paragraph_format.space_after = Pt(6)

    return doc


def add_formatted_paragraph(doc, text, style='Normal', bold=False, italic=False, alignment=None):
    """Add a paragraph with inline formatting (bold/italic markers)."""
    p = doc.add_paragraph(style=style)
    if alignment:
        p.alignment = alignment

    # Parse inline formatting: **bold**, *italic*, `code`
    parts = re.split(r'(\*\*.*?\*\*|\*.*?\*|`[^`]+`)', text)
    for part in parts:
        if not part:
            continue
        run = p.add_run()
        # Bold
        if part.startswith('**') and part.endswith('**'):
            run.text = part[2:-2]
            run.bold = True
        # Italic
        elif part.startswith('*') and part.endswith('*') and not part.startswith('**'):
            run.text = part[1:-1]
            run.italic = True
        # Inline code
        elif part.startswith('`') and part.endswith('`'):
            run.text = part[1:-1]
            run.font.name = "Courier New"
            run.font.size = Pt(10)
        else:
            run.text = part
            run.bold = bold
            run.italic = italic

        run.font.name = FONT_NAME
        run.font.size = FONT_SIZE_BODY

    return p


def add_code_block(doc, code_lines):
    """Add a code block with monospace font and light gray background."""
    for line in code_lines:
        p = doc.add_paragraph()
        p.paragraph_format.line_spacing = 1.0
        p.paragraph_format.space_before = Pt(0)
        p.paragraph_format.space_after = Pt(0)
        run = p.add_run(line)
        run.font.name = "Courier New"
        run.font.size = Pt(9)


def add_table(doc, headers, rows):
    """Add a formatted table."""
    table = doc.add_table(rows=1 + len(rows), cols=len(headers))
    table.style = 'Table Grid'

    # Headers
    for i, header in enumerate(headers):
        cell = table.rows[0].cells[i]
        cell.text = ''
        p = cell.paragraphs[0]
        run = p.add_run(header.strip().replace('**', ''))
        run.bold = True
        run.font.name = FONT_NAME
        run.font.size = Pt(10)

    # Rows
    for row_idx, row in enumerate(rows):
        for col_idx, cell_text in enumerate(row):
            cell = table.rows[row_idx + 1].cells[col_idx]
            cell.text = ''
            p = cell.paragraphs[0]
            # Remove bold markers
            clean_text = cell_text.strip().replace('**', '')
            run = p.add_run(clean_text)
            run.font.name = FONT_NAME
            run.font.size = Pt(10)

    doc.add_paragraph()  # spacing after table


def parse_table(lines, start_idx):
    """Parse a markdown table starting at start_idx. Returns (headers, rows, end_idx)."""
    headers = [c.strip() for c in lines[start_idx].split('|')[1:-1]]

    rows = []
    idx = start_idx + 2  # skip header separator line
    while idx < len(lines) and lines[idx].strip().startswith('|'):
        cells = [c.strip() for c in lines[idx].split('|')[1:-1]]
        rows.append(cells)
        idx += 1

    return headers, rows, idx


def convert():
    """Main conversion function."""
    md_text = INPUT_MD.read_text(encoding='utf-8')
    lines = md_text.split('\n')

    doc = setup_document()

    i = 0
    in_code_block = False
    code_lines = []

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        # Skip empty lines
        if not stripped:
            i += 1
            continue

        # HTML comments (figure placeholders) — skip entirely
        if stripped.startswith('<!--'):
            while i < len(lines) and '-->' not in lines[i]:
                i += 1
            i += 1
            continue

        # Horizontal rules
        if stripped == '---':
            i += 1
            continue

        # Code blocks
        if stripped.startswith('```'):
            if in_code_block:
                # End of code block
                add_code_block(doc, code_lines)
                code_lines = []
                in_code_block = False
            else:
                in_code_block = True
            i += 1
            continue

        if in_code_block:
            code_lines.append(line)
            i += 1
            continue

        # Headings
        if stripped.startswith('#') and not stripped.startswith('##'):
            # Heading 1
            heading_text = stripped.lstrip('#').strip()
            # Remove bold markers
            heading_text = heading_text.replace('**', '')
            p = doc.add_heading(heading_text, level=1)
            i += 1
            continue

        if stripped.startswith('## ') and not stripped.startswith('### '):
            heading_text = stripped.lstrip('#').strip()
            heading_text = heading_text.replace('**', '')
            p = doc.add_heading(heading_text, level=2)
            i += 1
            continue

        if stripped.startswith('### ') and not stripped.startswith('#### '):
            heading_text = stripped.lstrip('#').strip()
            heading_text = heading_text.replace('**', '')
            p = doc.add_heading(heading_text, level=3)
            i += 1
            continue

        if stripped.startswith('#### '):
            heading_text = stripped.lstrip('#').strip()
            heading_text = heading_text.replace('**', '')
            p = doc.add_heading(heading_text, level=4)
            i += 1
            continue

        # Tables
        if stripped.startswith('|') and i + 1 < len(lines) and '---' in lines[i + 1]:
            headers, rows, end_idx = parse_table(lines, i)
            add_table(doc, headers, rows)
            i = end_idx
            continue

        # Bullet lists
        if stripped.startswith('- ') or stripped.startswith('* '):
            text = stripped[2:].strip()
            p = doc.add_paragraph(style='List Bullet')
            # Parse inline formatting
            parts = re.split(r'(\*\*.*?\*\*|\*.*?\*|`[^`]+`)', text)
            for part in parts:
                if not part:
                    continue
                run = p.add_run()
                if part.startswith('**') and part.endswith('**'):
                    run.text = part[2:-2]
                    run.bold = True
                elif part.startswith('*') and part.endswith('*') and not part.startswith('**'):
                    run.text = part[1:-1]
                    run.italic = True
                elif part.startswith('`') and part.endswith('`'):
                    run.text = part[1:-1]
                    run.font.name = "Courier New"
                    run.font.size = Pt(10)
                else:
                    run.text = part
                run.font.name = FONT_NAME
                run.font.size = FONT_SIZE_BODY
            i += 1
            continue

        # Numbered lists
        num_match = re.match(r'^(\d+)\.\s+(.*)', stripped)
        if num_match:
            text = num_match.group(2).strip()
            p = doc.add_paragraph(style='List Number')
            parts = re.split(r'(\*\*.*?\*\*|\*.*?\*|`[^`]+`)', text)
            for part in parts:
                if not part:
                    continue
                run = p.add_run()
                if part.startswith('**') and part.endswith('**'):
                    run.text = part[2:-2]
                    run.bold = True
                elif part.startswith('*') and part.endswith('*') and not part.startswith('**'):
                    run.text = part[1:-1]
                    run.italic = True
                elif part.startswith('`') and part.endswith('`'):
                    run.text = part[1:-1]
                    run.font.name = "Courier New"
                    run.font.size = Pt(10)
                else:
                    run.text = part
                run.font.name = FONT_NAME
                run.font.size = FONT_SIZE_BODY
            i += 1
            continue

        # Image captions (lines starting with **Gambar)
        if stripped.startswith('**Gambar') or stripped.startswith('**gambar'):
            text = stripped.replace('**', '')
            p = doc.add_paragraph()
            p.alignment = WD_ALIGN_PARAGRAPH.CENTER
            run = p.add_run(text)
            run.font.name = FONT_NAME
            run.font.size = Pt(10)
            run.italic = True
            i += 1
            continue

        # Italic-only lines (e.g., "*This section is written in English...*")
        if stripped.startswith('*') and stripped.endswith('*') and not stripped.startswith('**'):
            text = stripped[1:-1]
            p = doc.add_paragraph()
            run = p.add_run(text)
            run.font.name = FONT_NAME
            run.font.size = FONT_SIZE_BODY
            run.italic = True
            i += 1
            continue

        # Normal paragraph with inline formatting
        add_formatted_paragraph(doc, stripped)
        i += 1

    # Save
    doc.save(str(OUTPUT_DOCX))
    print(f"[OK] Converted: {INPUT_MD.name} -> {OUTPUT_DOCX.name}")
    print(f"     Output: {OUTPUT_DOCX}")


if __name__ == "__main__":
    convert()
