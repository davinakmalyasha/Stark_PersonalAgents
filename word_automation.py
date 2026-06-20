"""
Word Automation Module — Opens Microsoft Word visibly on screen,
creates a new document, writes markdown-formatted content, saves as .docx, and closes.
"""
import os
import re
from state_sync import shared_state, safe_append_live_log


def append_inline_formatted(word_range, text: str):
    """Appends text to a collapsed range, supporting bold and italic markdown tags."""
    # Split by bold tags '**'
    bold_parts = text.split("**")
    for idx, b_part in enumerate(bold_parts):
        is_bold = (idx % 2 == 1)
        # Split by italic tags '*'
        italic_parts = b_part.split("*")
        for s_idx, i_part in enumerate(italic_parts):
            is_italic = (s_idx % 2 == 1)
            
            if not i_part:
                continue
                
            # Collapse range to end and insert text segment
            word_range.Collapse(0)  # 0 = wdCollapseEnd
            word_range.Text = i_part
            word_range.Font.Bold = is_bold
            word_range.Font.Italic = is_italic


def write_docx_with_word(filepath: str, title: str, body_text: str) -> bool:
    """
    Automates Microsoft Word to create, format, and save a document from Markdown.
    Translates headings, lists, tables, code blocks, and inline bold/italic tags.
    """
    import win32com.client
    import pythoncom

    basename = os.path.basename(filepath)
    safe_append_live_log(f"\n[Word Automation] Launching Microsoft Word to generate {basename}...")
    pythoncom.CoInitialize()
    word = None
    try:
        word = win32com.client.Dispatch("Word.Application")
        word.Visible = True

        doc = word.Documents.Add()

        # Write Title at the very beginning of the document using doc.Content
        r_title = doc.Content
        r_title.Text = title
        r_title.Style = "Heading 1"
        r_title.Font.Size = 18
        r_title.Font.Bold = True

        # Parse markdown body line-by-line
        lines = body_text.splitlines()
        idx = 0
        in_code_block = False
        code_lines = []

        while idx < len(lines):
            line = lines[idx].strip()

            # Skip empty lines outside code blocks
            if not line and not in_code_block:
                idx += 1
                continue

            # 1. Code Blocks
            if line.startswith("```"):
                in_code_block = not in_code_block
                if not in_code_block and code_lines:
                    # End of code block: insert monospaced code text at the collapsed end
                    doc.Content.InsertAfter("\n")
                    r = doc.Content
                    r.Collapse(0)
                    r.Text = "\n".join(code_lines)
                    r.Font.Name = "Courier New"
                    r.Font.Size = 9.5
                    r.ParagraphFormat.LeftIndent = 24  # indent in points
                    r.ParagraphFormat.SpaceAfter = 6
                    code_lines = []
                idx += 1
                continue

            if in_code_block:
                # Add original line (preserve indentation)
                code_lines.append(lines[idx])
                idx += 1
                continue

            # 2. Table blocks
            if line.startswith("|"):
                table_lines = []
                while idx < len(lines) and lines[idx].strip().startswith("|"):
                    table_lines.append(lines[idx].strip())
                    idx += 1

                # Parse and clean row arrays
                parsed_rows = []
                for t_line in table_lines:
                    # Split cells by | and strip spaces
                    cells = [c.strip() for c in t_line.split("|")[1:-1]]
                    # Ignore separator rows like | --- | --- |
                    if all(re.match(r'^[\s\-:]+$', c) for c in cells):
                        continue
                    parsed_rows.append(cells)

                if parsed_rows:
                    num_rows = len(parsed_rows)
                    num_cols = max(len(r) for r in parsed_rows)

                    doc.Content.InsertAfter("\n")
                    table_range = doc.Content
                    table_range.Collapse(0)
                    table = doc.Tables.Add(table_range, NumRows=num_rows, NumColumns=num_cols)
                    table.Style = "Table Grid"

                    # Populate cells and style header
                    for r_idx, row in enumerate(parsed_rows):
                        for c_idx, val in enumerate(row):
                            if c_idx < num_cols:
                                cell = table.Cell(r_idx + 1, c_idx + 1)
                                if r_idx == 0:
                                    cell.Range.Text = val
                                    cell.Range.Font.Bold = True
                                    cell.Shading.BackgroundPatternColor = 15790320  # light gray
                                else:
                                    # Support inline formatting in cells
                                    r = cell.Range
                                    r.End = r.End - 1
                                    r.Text = ""
                                    append_inline_formatted(r, val)

                    # Add trailing spacer paragraph break
                    doc.Content.InsertAfter("\n")
                continue

            # 3. Headings
            heading_match = re.match(r'^(#+)\s+(.+)$', line)
            if heading_match:
                level = len(heading_match.group(1))
                text = heading_match.group(2)
                
                doc.Content.InsertAfter("\n")
                r = doc.Content
                r.Collapse(0)
                r.Text = text
                if level == 1:
                    r.Style = "Heading 1"
                elif level == 2:
                    r.Style = "Heading 2"
                else:
                    r.Style = "Heading 3"
                idx += 1
                continue

            # 4. Bullet lists
            list_match = re.match(r'^[\-\*]\s+(.+)$', line)
            if list_match:
                text = list_match.group(1)
                
                doc.Content.InsertAfter("\n")
                r = doc.Content
                r.Collapse(0)
                r.Style = "List Bullet"
                append_inline_formatted(r, text)
                idx += 1
                continue

            # 5. Normal text paragraphs
            doc.Content.InsertAfter("\n")
            r = doc.Content
            r.Collapse(0)
            r.Style = "Normal"
            r.ParagraphFormat.SpaceAfter = 6
            append_inline_formatted(r, line)
            idx += 1

        # Save the Word Document
        abs_path = os.path.abspath(filepath).replace("/", "\\")
        doc.SaveAs(abs_path, FileFormat=16)  # 16 = wdFormatDocumentDefault (.docx)
        doc.Close()

        safe_append_live_log(f"[Word Automation] [OK] Successfully generated formatted Word document: {basename}")
        return True
    except Exception as e:
        import traceback
        tb_str = traceback.format_exc()
        err = f"[Word Automation] [ERROR] Failed to compile Word document: {e}\nTraceback:\n{tb_str}"
        safe_append_live_log(err)
        print(err.encode('ascii', errors='replace').decode('ascii'))
        return False
    finally:
        try:
            if word is not None:
                word.Quit()
        except Exception:
            pass
        pythoncom.CoUninitialize()
