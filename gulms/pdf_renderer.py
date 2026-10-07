"""
GULMS PDF Renderer.
Converts Markdown study notes into beautifully styled, vector-quality A4 PDFs
with native Mermaid SVG diagrams, clean tables, embedded technical figures,
and JetBrains Mono code blocks, fully optimized for reading in Zathura and print.
"""

import base64
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Optional


def _format_inline(text: str, base_dir: Optional[Path] = None) -> str:
    """Formats inline Markdown: embedded base64 images, bold, italics, inline code."""
    # 1. Images: ![alt](url)
    def _img_replace(match):
        alt = match.group(1)
        src = match.group(2)
        final_src = src
        if base_dir and not (src.startswith("http://") or src.startswith("https://") or src.startswith("data:")):
            img_path = (Path(base_dir) / src).resolve()
            if img_path.exists():
                ext = img_path.suffix.lower()
                mime_map = {
                    ".png": "image/png",
                    ".jpg": "image/jpeg",
                    ".jpeg": "image/jpeg",
                    ".svg": "image/svg+xml",
                    ".webp": "image/webp",
                    ".gif": "image/gif"
                }
                mime = mime_map.get(ext, "image/png")
                b64 = base64.b64encode(img_path.read_bytes()).decode("ascii")
                final_src = f"data:{mime};base64,{b64}"
        caption_html = f"<figcaption>{alt}</figcaption>" if alt else ""
        return f'<figure class="doc-figure"><img src="{final_src}" alt="{alt}">{caption_html}</figure>'

    text = re.sub(r"!\[(.*?)\]\((.*?)\)", _img_replace, text)
    # 2. Bold
    text = re.sub(r"\*\*(.*?)\*\*", r"<strong>\1</strong>", text)
    # 3. Italic (asterisk)
    text = re.sub(r"(?<!\*)\*(?!\*)(.*?)(?<!\*)\*(?!\*)", r"<em>\1</em>", text)
    # 4. Inline code
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    return text


def markdown_to_html(md_text: str, title: str = "Lecture Notes", base_dir: Optional[Path] = None) -> str:
    """Translates Markdown text with Mermaid, tables, and images into standalone styled HTML."""
    html_lines = []
    in_code = False
    code_lang = ""
    code_buf = []

    in_mermaid = False
    mermaid_buf = []

    in_table = False
    table_buf = []

    in_list = False

    for line in md_text.splitlines():
        # Code fence
        if line.startswith("```"):
            if in_code:
                in_code = False
                safe_code = "\n".join(code_buf).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
                html_lines.append(f'<pre><code class="language-{code_lang}">{safe_code}</code></pre>')
                code_buf = []
                continue
            elif in_mermaid:
                in_mermaid = False
                diag = "\n".join(mermaid_buf)
                html_lines.append(f'<div class="mermaid">\n{diag}\n</div>')
                mermaid_buf = []
                continue
            else:
                tag = line.strip()[3:].strip()
                if tag == "mermaid":
                    in_mermaid = True
                    mermaid_buf = []
                else:
                    in_code = True
                    code_lang = tag
                    code_buf = []
                continue

        if in_code:
            code_buf.append(line)
            continue
        if in_mermaid:
            mermaid_buf.append(line)
            continue

        # Tables
        if line.strip().startswith("|") and line.strip().endswith("|"):
            if not in_table:
                in_table = True
                table_buf = []
            table_buf.append(line.strip())
            continue
        elif in_table:
            in_table = False
            html_lines.append("<table>")
            rows = [r.strip("|").split("|") for r in table_buf if not re.match(r"^\|[\s\-:]+\|$", r)]
            if rows:
                html_lines.append("  <thead><tr>" + "".join(f"<th>{_format_inline(c.strip(), base_dir=base_dir)}</th>" for c in rows[0]) + "</tr></thead>")
                html_lines.append("  <tbody>")
                for r in rows[1:]:
                    html_lines.append("    <tr>" + "".join(f"<td>{_format_inline(c.strip(), base_dir=base_dir)}</td>" for c in r) + "</tr>")
                html_lines.append("  </tbody>")
            html_lines.append("</table>")
            table_buf = []

        # Bullet Lists
        if re.match(r"^\s*-\s+", line):
            if not in_list:
                in_list = True
                html_lines.append("<ul>")
            content = re.sub(r"^\s*-\s+", "", line)
            content = _format_inline(content, base_dir=base_dir)
            html_lines.append(f"  <li>{content}</li>")
            continue
        elif in_list and line.strip() == "":
            html_lines.append("</ul>")
            in_list = False
            continue

        # Headings
        if line.startswith("# "):
            if in_list: html_lines.append("</ul>"); in_list = False
            html_lines.append(f"<h1>{_format_inline(line[2:].strip(), base_dir=base_dir)}</h1>")
            continue
        elif line.startswith("## "):
            if in_list: html_lines.append("</ul>"); in_list = False
            html_lines.append(f"<h2>{_format_inline(line[3:].strip(), base_dir=base_dir)}</h2>")
            continue
        elif line.startswith("### "):
            if in_list: html_lines.append("</ul>"); in_list = False
            html_lines.append(f"<h3>{_format_inline(line[4:].strip(), base_dir=base_dir)}</h3>")
            continue

        # Blockquote
        if line.startswith("> "):
            if in_list: html_lines.append("</ul>"); in_list = False
            raw_content = line[2:].strip()
            # If the blockquote contains an image, format directly
            if raw_content.startswith("![") and raw_content.endswith(")"):
                html_lines.append(_format_inline(raw_content, base_dir=base_dir))
            else:
                content = _format_inline(raw_content, base_dir=base_dir)
                html_lines.append(f"<blockquote>{content}</blockquote>")
            continue

        # Horizontal rule
        if line.strip() in ("---", "***"):
            if in_list: html_lines.append("</ul>"); in_list = False
            html_lines.append("<hr>")
            continue

        # Empty line
        if not line.strip():
            if in_list: html_lines.append("</ul>"); in_list = False
            continue

        # Standalone Image
        if line.strip().startswith("![") and line.strip().endswith(")"):
            if in_list: html_lines.append("</ul>"); in_list = False
            html_lines.append(_format_inline(line.strip(), base_dir=base_dir))
            continue

        # Standard paragraph
        content = _format_inline(line, base_dir=base_dir)
        html_lines.append(f"<p>{content}</p>")

    if in_list: html_lines.append("</ul>")
    if in_table: html_lines.append("</table>")

    body_html = "\n".join(html_lines)

    return f"""<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>{title}</title>
<script src="https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.min.js"></script>
<script>
mermaid.initialize({{ startOnLoad: true, theme: 'neutral', securityLevel: 'loose' }});
</script>
<style>
@import url('https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap');

@page {{
    size: A4;
    margin: 20mm 18mm 20mm 18mm;
    @bottom-right {{
        content: counter(page);
        font-family: 'Inter', sans-serif;
        font-size: 8.5pt;
        color: #94a3b8;
    }}
}}

body {{
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    font-size: 10.5pt;
    line-height: 1.65;
    color: #1e293b;
    background-color: #ffffff;
    max-width: 840px;
    margin: 0 auto;
}}

h1 {{
    font-size: 21pt;
    font-weight: 700;
    color: #0f172a;
    border-bottom: 2px solid #e2e8f0;
    padding-bottom: 8px;
    margin-top: 32px;
    margin-bottom: 16px;
    page-break-after: avoid;
}}

h2 {{
    font-size: 15pt;
    font-weight: 600;
    color: #1e293b;
    margin-top: 28px;
    margin-bottom: 12px;
    border-bottom: 1px solid #f1f5f9;
    padding-bottom: 4px;
    page-break-after: avoid;
}}

h3 {{
    font-size: 12pt;
    font-weight: 600;
    color: #334155;
    margin-top: 20px;
    margin-bottom: 8px;
    page-break-after: avoid;
}}

p {{
    margin-top: 0;
    margin-bottom: 10px;
}}

ul {{
    padding-left: 22px;
    margin-top: 0;
    margin-bottom: 14px;
}}

li {{
    margin-bottom: 5px;
}}

table {{
    width: 100%;
    border-collapse: collapse;
    margin: 18px 0;
    font-size: 9.5pt;
    page-break-inside: avoid;
}}

th, td {{
    border: 1px solid #cbd5e1;
    padding: 8px 12px;
    text-align: left;
    vertical-align: top;
}}

th {{
    background-color: #f1f5f9;
    font-weight: 600;
    color: #0f172a;
    border-bottom: 2px solid #94a3b8;
}}

tr:nth-child(even) td {{
    background-color: #f8fafc;
}}

blockquote {{
    border-left: 3.5px solid #3b82f6;
    background: #f0f7ff;
    margin: 14px 0;
    padding: 10px 14px;
    border-radius: 0 6px 6px 0;
    color: #1e40af;
    font-size: 10pt;
    page-break-inside: avoid;
}}

hr {{
    border: none;
    border-top: 1px solid #e2e8f0;
    margin: 28px 0;
}}

.mermaid {{
    display: flex;
    justify-content: center;
    background: #ffffff;
    border: 1px solid #e2e8f0;
    border-radius: 6px;
    padding: 16px;
    margin: 20px 0;
    page-break-inside: avoid;
}}

figure.doc-figure {{
    margin: 20px auto;
    text-align: center;
    page-break-inside: avoid;
}}

figure.doc-figure img {{
    max-width: 95%;
    max-height: 480px;
    height: auto;
    border-radius: 6px;
    border: 1px solid #cbd5e1;
    box-shadow: 0 1px 3px rgba(0,0,0,0.05);
    background: #ffffff;
    display: inline-block;
}}

figure.doc-figure figcaption {{
    font-size: 8.5pt;
    color: #64748b;
    margin-top: 6px;
    font-style: italic;
}}

code, pre {{
    font-family: 'JetBrains Mono', monospace;
    font-size: 9pt;
}}

pre {{
    background: #f8fafc;
    color: #0f172a;
    border: 1px solid #e2e8f0;
    padding: 14px 16px;
    border-radius: 6px;
    overflow-x: auto;
    line-height: 1.5;
    page-break-inside: avoid;
}}

p code, li code, td code {{
    background: #f1f5f9;
    color: #0f172a;
    padding: 2px 5px;
    border-radius: 4px;
    border: 1px solid #e2e8f0;
}}
</style>
</head>
<body>
{body_html}
</body>
</html>
"""


def inject_pdf_outline(pdf_path: Path, title: str = "Lecture Notes") -> bool:
    """Injects Table of Contents outline bookmarks into the generated PDF for Zathura navigation."""
    pdf_path = Path(pdf_path)
    if not pdf_path.exists():
        return False

    # 1. Try local environment
    try:
        import pymupdf as fitz
        doc = fitz.open(str(pdf_path))
        toc = [[1, title, 1]]
        for pno in range(len(doc)):
            page_text = doc[pno].get_text()
            for line in page_text.splitlines():
                line = line.strip()
                if line.startswith("Slide "):
                    toc.append([2, line, pno + 1])
        if len(toc) > 1:
            doc.set_toc(toc)
        doc.set_metadata({
            "title": title,
            "author": "GULMS",
            "subject": "Lecture Study Notes",
            "creator": "GULMS Vector PDF Engine"
        })
        doc.saveIncr()
        doc.close()
        return True
    except (ImportError, Exception):
        pass

    # 2. Try media-python bridge if pymupdf is in media venv
    media_python = None
    env_bin = os.environ.get("MEDIA_PYTHON_BIN")
    if env_bin and os.path.exists(env_bin):
        media_python = env_bin
    elif shutil.which("media-python"):
        media_python = shutil.which("media-python")
    elif (Path.home() / ".local" / "bin" / "media-python").exists():
        media_python = str(Path.home() / ".local" / "bin" / "media-python")

    if media_python:
        code = """import sys
from pathlib import Path

pdf_file = Path(sys.argv[1])
title = sys.argv[2]
if not pdf_file.exists():
    sys.exit(1)

try:
    import pymupdf as fitz
except ImportError:
    try:
        import fitz
    except ImportError:
        sys.exit(2)

doc = fitz.open(str(pdf_file))
toc = [[1, title, 1]]

for pno in range(len(doc)):
    page_text = doc[pno].get_text()
    for line in page_text.splitlines():
        line = line.strip()
        if line.startswith("Slide "):
            toc.append([2, line, pno + 1])

if len(toc) > 1:
    doc.set_toc(toc)

doc.set_metadata({
    "title": title,
    "author": "GULMS",
    "subject": "Lecture Study Notes",
    "creator": "GULMS Vector PDF Engine"
})
doc.saveIncr()
doc.close()
"""
        try:
            res = subprocess.run([str(media_python), "-c", code, str(pdf_path), title], capture_output=True, text=True, timeout=8)
            return res.returncode == 0
        except Exception:
            return False

    return False


def render_markdown_to_pdf(
    md_text: str,
    out_pdf_path: Path,
    title: str = "Lecture Notes",
    base_dir: Optional[Path] = None
) -> Path:
    """Renders Markdown into a vector PDF via headless Chromium/Chrome with TOC outline injection."""
    out_pdf_path = Path(out_pdf_path)
    out_pdf_path.parent.mkdir(parents=True, exist_ok=True)

    # Detect browser binary
    chrome_bin = os.environ.get("CHROME_BIN")
    if not chrome_bin or not shutil.which(chrome_bin):
        for candidate in ["chromium", "google-chrome", "google-chrome-stable", "brave", "microsoft-edge"]:
            found = shutil.which(candidate)
            if found:
                chrome_bin = found
                break

    if not chrome_bin:
        raise RuntimeError(
            "No compatible headless browser (chromium/google-chrome) found on PATH for PDF generation. "
            "Please install chromium or set CHROME_BIN=/path/to/browser."
        )

    html_content = markdown_to_html(md_text, title=title, base_dir=base_dir)

    with tempfile.NamedTemporaryFile("w", suffix=".html", delete=False, encoding="utf-8") as tf:
        tf.write(html_content)
        tmp_html = tf.name

    try:
        cmd = [
            chrome_bin,
            "--headless=new",
            "--no-sandbox",
            "--virtual-time-budget=4000",
            "--no-pdf-header-footer",
            f"--print-to-pdf={out_pdf_path}",
            tmp_html,
        ]
        res = subprocess.run(cmd, capture_output=True, text=True)
        if res.returncode != 0:
            raise RuntimeError(f"Browser PDF rendering failed (exit code {res.returncode}): {res.stderr or res.stdout}")
    finally:
        if os.path.exists(tmp_html):
            os.remove(tmp_html)

    # Post-process with TOC bookmarks for Zathura
    if out_pdf_path.exists():
        inject_pdf_outline(out_pdf_path, title=title)

    return out_pdf_path


def open_in_zathura(pdf_path: Path):
    """Spawns Zathura in background to view the generated PDF, falling back to xdg-open."""
    zathura_bin = shutil.which("zathura")
    if zathura_bin:
        subprocess.Popen(
            [zathura_bin, str(pdf_path)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True
        )
    else:
        xdg_bin = shutil.which("xdg-open")
        if xdg_bin:
            subprocess.Popen(
                [xdg_bin, str(pdf_path)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True
            )
        else:
            print(f"[INFO] Neither zathura nor xdg-open was found to view PDF. Saved to: {pdf_path}")
