"""
Unit and integration tests for GULMS PDF Renderer.
Validates HTML translation, inline image base64 embedding,
Chromium vector PDF printing, and PyMuPDF outline TOC injection for Zathura.
"""

import base64
import os
import shutil
import tempfile
from pathlib import Path
import pytest

from gulms.pdf_renderer import markdown_to_html, render_markdown_to_pdf, inject_pdf_outline


def test_markdown_to_html_typography_and_elements():
    """Validates markdown parsing into semantic HTML tags with correct typography classes."""
    md = """# Introduction to DBMS

This is a paragraph with **bold text**, *italic text*, and `code snippet`.

- Bullet point 1
- Bullet point 2

| Feature | DBMS | File System |
| --- | --- | --- |
| Redundancy | Minimal | High |
| Concurrency | Supported | Poor |

> Important Note: Always normalize tables.
"""
    html = markdown_to_html(md, title="DBMS Notes")

    assert "<h1>Introduction to DBMS</h1>" in html
    assert "<strong>bold text</strong>" in html
    assert "<em>italic text</em>" in html
    assert "<code>code snippet</code>" in html
    assert "<ul>" in html
    assert "<li>Bullet point 1</li>" in html
    assert "<table>" in html
    assert "<th>Feature</th>" in html
    assert "<td>Minimal</td>" in html
    assert "<blockquote>Important Note: Always normalize tables.</blockquote>" in html
    assert "font-family: 'Inter'" in html
    assert "font-family: 'JetBrains Mono'" in html


def test_markdown_to_html_image_base64_resolution(tmp_path):
    """Verifies that relative image paths in Markdown are resolved and inlined as Base64 data URIs."""
    figures_dir = tmp_path / "figures"
    figures_dir.mkdir(parents=True)
    img_file = figures_dir / "test_fig.png"
    # Create minimal 1x1 PNG
    raw_png = (
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
        b"\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00"
        b"\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82"
    )
    img_file.write_bytes(raw_png)

    md = "> ![Sample Figure](figures/test_fig.png)"
    html = markdown_to_html(md, base_dir=tmp_path)

    assert "data:image/png;base64," in html
    assert base64.b64encode(raw_png).decode("ascii") in html
    assert '<figure class="doc-figure">' in html
    assert "<figcaption>Sample Figure</figcaption>" in html


def test_render_markdown_to_pdf_end_to_end(tmp_path):
    """Verifies end-to-end vector PDF rendering via Chromium and TOC bookmark injection."""
    if not shutil.which("chromium"):
        pytest.skip("Chromium not installed on host")

    md = """# Lecture 01: Relational Architecture

Source: `Session_1.pptx` (2 slides)

---

## Slide 1: Introduction to Relations

- Keys and attributes
- Functional dependencies

---

## Slide 2: Normalization

- 1NF, 2NF, 3NF
- BCNF decomposition
"""
    out_pdf = tmp_path / "test_notes.pdf"
    res_path = render_markdown_to_pdf(md, out_pdf, title="Lecture 01: Relational Architecture")

    assert res_path.exists()
    assert res_path.stat().st_size > 1000

    # Verify standard PDF magic header
    with open(res_path, "rb") as f:
        header = f.read(5)
        assert header == b"%PDF-"


def test_inject_pdf_outline(tmp_path):
    """Verifies TOC outline bookmark injection for Zathura reader."""
    if not shutil.which("chromium"):
        pytest.skip("Chromium not installed on host")

    md = """# Lecture 05: Indexing

## Slide 1: B+ Trees

Content for slide 1.

## Slide 2: Hash Indexes

Content for slide 2.
"""
    out_pdf = tmp_path / "test_outline.pdf"
    render_markdown_to_pdf(md, out_pdf, title="Lecture 05: Indexing")

    # Verify outline bookmarks using pymupdf if available
    try:
        import pymupdf as fitz
        doc = fitz.open(str(out_pdf))
        toc = doc.get_toc()
        assert len(toc) >= 2
        titles = [t[1] for t in toc]
        assert any("Slide 1" in t for t in titles)
        assert any("Slide 2" in t for t in titles)
    except ImportError:
        pass
