"""
GULMS Slide-to-Markdown Extraction & Diagram Understanding Engine.
Translates academic PowerPoint presentations into clean, study-ready Markdown
with connected Mermaid flowcharts, structured tables, and academic figure cards.
"""

import sys
import os
import io
import re
import math
import hashlib
import subprocess
import shutil
from pathlib import Path
from typing import List, Dict, Tuple, Optional, Any, Set
import xml.etree.ElementTree as ET

# OpenXML Namespaces
NS = {
    'p': 'http://schemas.openxmlformats.org/presentationml/2006/main',
    'a': 'http://schemas.openxmlformats.org/drawingml/2006/main',
    'r': 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
}

# Pre-computed Corpus-Level SHA-256 Hashes of University Template Assets
# Identified from empirical cross-deck analysis across Galgotias course decks
TEMPLATE_CORPUS_PREFIXES = {
    "305ff7cbda57d943",  # G-SCALE 640x640 badge
    "647386bf650a0065",  # Galgotias Crest header banner
    "4dc6cc05a8b1d55f",  # Think-Pair-Share banner
    "0f70c06e01d10737",  # Title slide background template
    "551b16443b938701",  # Course outcomes badge
    "ca399f9ba7eec999",  # Small university emblem
    "249ed19ed3a885e2",  # Small Think-Pair-Share icon
    "b4f99aeb9e9b234c",  # Template watermark
}

# Regex for template chrome text
TEMPLATE_TEXT_PATTERNS = [
    r"galgotias\s+university",
    r"school\s+of\s+computing\s+science",
    r"department\s+of\s+computer\s+science",
    r"\bg-?scale\b",
    r"student-?centred\s+active\s+learning\s+ecosystem",
    r"think-?pair-?share",
    r"course\s+code\s*:\s*[A-Z0-9]+",
    r"session\s*:\s*202\d",
    r"program\s*:\s*b\.?tech",
    r"^\s*\[\s*\d+\s*-?\s*mins?\s*\]\s*$",
    r"^\s*\d+\s*$",  # Standalone slide number
]
TEMPLATE_TEXT_RE = re.compile("|".join(TEMPLATE_TEXT_PATTERNS), re.IGNORECASE)


def is_template_text(text: str) -> bool:
    """Returns True if the text represents chrome, university header, or slide counter."""
    cleaned = text.strip()
    if not cleaned or len(cleaned) < 2:
        return True
    return bool(TEMPLATE_TEXT_RE.search(cleaned))


def is_template_asset(img_bytes: bytes, ocr_text: str = "") -> bool:
    """Zero-heuristic template image detection based on SHA-256 prefix or verified OCR text."""
    h = hashlib.sha256(img_bytes).hexdigest()
    if any(h.startswith(prefix) for prefix in TEMPLATE_CORPUS_PREFIXES):
        return True
    if ocr_text:
        low = ocr_text.lower()
        if any(phrase in low for phrase in ("active learning ecosystem", "student-centred active", "galgotias university")):
            return True
    return False


def dist_to_bbox(point: Tuple[float, float], bbox: Tuple[float, float, float, float]) -> float:
    """Euclidean distance from a 2D point to an axis-aligned bounding box."""
    x, y, w, h = bbox
    px, py = point
    dx = max(x - px, 0.0, px - (x + w))
    dy = max(y - py, 0.0, py - (y + h))
    return math.hypot(dx, dy)


class SlideExtractor:
    """Production slide-to-markdown extraction engine."""

    def __init__(self, media_python_bin: Optional[str] = None):
        if media_python_bin:
            self.media_python_bin = media_python_bin
        else:
            env_bin = os.environ.get("MEDIA_PYTHON_BIN")
            if env_bin and os.path.exists(env_bin):
                self.media_python_bin = env_bin
            elif shutil.which("media-python"):
                self.media_python_bin = shutil.which("media-python")
            elif (Path.home() / ".local" / "bin" / "media-python").exists():
                self.media_python_bin = str(Path.home() / ".local" / "bin" / "media-python")
            else:
                self.media_python_bin = sys.executable

    def extract_deck(self, pptx_bytes: bytes, out_dir: Optional[Path] = None) -> List[Dict[str, Any]]:
        """Extract all slides from presentation bytes into clean structured data & Markdown."""
        try:
            import pptx
        except ImportError:
            # Dispatch to isolated media-python environment
            return self._extract_via_media_bridge(pptx_bytes, out_dir)

        from pptx import Presentation
        from pptx.enum.shapes import MSO_SHAPE_TYPE

        prs = Presentation(io.BytesIO(pptx_bytes))
        total_slides = len(prs.slides)
        deck_results = []

        # Step 1: Precompute deck-level shape frequency for layout invariance
        shape_frequencies = self._compute_deck_frequencies(prs)

        # Prepare figures output directory if specified
        fig_dir = (out_dir / "figures") if out_dir else None
        if fig_dir:
            fig_dir.mkdir(parents=True, exist_ok=True)

        for s_idx, slide in enumerate(prs.slides, start=1):
            slide_data = self._extract_single_slide(
                slide=slide,
                slide_num=s_idx,
                total_slides=total_slides,
                shape_frequencies=shape_frequencies,
                fig_dir=fig_dir
            )
            deck_results.append(slide_data)

        return deck_results

    def _compute_deck_frequencies(self, prs) -> Dict[Tuple[int, int, int, int], float]:
        """Calculates spatial repetition of shapes across the deck (≥70% = deck chrome)."""
        counts = {}
        total = max(1, len(prs.slides))
        for slide in prs.slides:
            for shape in slide.shapes:
                # Bin positions by ~2mm (180,000 EMUs)
                bx = (shape.left // 180000, shape.top // 180000,
                      shape.width // 180000, shape.height // 180000)
                counts[bx] = counts.get(bx, 0) + 1
        return {bx: count / total for bx, count in counts.items()}

    def _extract_single_slide(
        self, slide, slide_num: int, total_slides: int,
        shape_frequencies: Dict[Tuple[int, int, int, int], float],
        fig_dir: Optional[Path]
    ) -> Dict[str, Any]:
        """Extracts content from a single slide across all semantic layers."""
        from pptx.enum.shapes import MSO_SHAPE_TYPE

        # 1. Resolve Title
        title = ""
        if slide.shapes.title and slide.shapes.title.has_text_frame and slide.shapes.title.text:
            raw_t = slide.shapes.title.text.strip()
            if not is_template_text(raw_t):
                title = re.sub(r'\s+', ' ', raw_t)

        # 2. Flatten grouped shapes recursively
        all_shapes = self._flatten_shapes(slide.shapes)

        text_blocks: List[Tuple[int, str]] = []
        tables: List[List[List[str]]] = []
        figures: List[Dict[str, Any]] = []

        # 3. Process DrawingML XML for topological graph synthesis
        graph_nodes, graph_edges = self._extract_diagram_graph(slide, all_shapes)

        # 4. Extract tables, pictures, and text
        for shape in all_shapes:
            # Check deck-level chrome frequency
            bx = (shape.left // 180000, shape.top // 180000,
                  shape.width // 180000, shape.height // 180000)
            if shape_frequencies.get(bx, 0.0) >= 0.70:
                continue  # Skip deck chrome

            # Native Tables
            if shape.has_table:
                tbl_data = []
                for row in shape.table.rows:
                    cells = [cell.text.strip().replace("\n", " ") for cell in row.cells]
                    if any(cells):
                        tbl_data.append(cells)
                if tbl_data:
                    tables.append(tbl_data)
                continue

            # Pictures / Figures
            if shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
                try:
                    blob = shape.image.blob
                    if not is_template_asset(blob):
                        fig_info = self._handle_figure(blob, slide_num, len(figures) + 1, fig_dir)
                        if fig_info:
                            figures.append(fig_info)
                except Exception:
                    pass
                continue

            # Standard Text Frames
            if shape.has_text_frame and shape != slide.shapes.title:
                # If shape is already a node in the diagram, avoid duplicating in bullet list
                if any(n["text"] == shape.text_frame.text.strip() for n in graph_nodes):
                    continue

                for p in shape.text_frame.paragraphs:
                    ptxt = p.text.strip()
                    if ptxt and not is_template_text(ptxt):
                        text_blocks.append((p.level, ptxt))

        # Fallback Title if empty
        if not title and text_blocks:
            cand = text_blocks[0][1]
            if len(cand) < 80 and not is_template_text(cand):
                title = cand
                text_blocks = text_blocks[1:]

        # Fallback OCR for Canva/Screenshot slides with 0 text frames
        if not text_blocks and not graph_nodes and not tables and figures:
            for fig in figures:
                if fig.get("ocr_lines") and len(fig["ocr_lines"]) >= 3:
                    if not title:
                        title = fig["ocr_lines"][0]
                        ocr_content = fig["ocr_lines"][1:]
                    else:
                        ocr_content = fig["ocr_lines"]
                    for line in ocr_content:
                        if not is_template_text(line):
                            text_blocks.append((0, line))

        # Build Clean Markdown Output
        md_text = self._render_markdown(title, text_blocks, tables, graph_nodes, graph_edges, figures)

        return {
            "slide_num": slide_num,
            "title": title,
            "text_blocks": text_blocks,
            "tables": tables,
            "graph_nodes": graph_nodes,
            "graph_edges": graph_edges,
            "figures": figures,
            "markdown": md_text
        }

    def _flatten_shapes(self, shape_collection) -> List[Any]:
        """Recursively unrolls <p:grpSp> containers to access all nested shapes."""
        from pptx.enum.shapes import MSO_SHAPE_TYPE
        flat = []
        for shape in shape_collection:
            if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
                flat.extend(self._flatten_shapes(shape.shapes))
            else:
                flat.append(shape)
        return flat

    def _extract_diagram_graph(self, slide, all_shapes) -> Tuple[List[Dict[str, Any]], List[Tuple[str, str, str]]]:
        """Extracts geometric shapes and connectors, routing lines to nearest nodes."""
        root = ET.fromstring(slide._element.xml)

        nodes = []
        lines = []
        labels = []

        for sp in root.findall('.//p:sp', NS):
            nv = sp.find('.//p:nvSpPr', NS)
            sp_id = nv.find('.//p:cNvPr', NS).attrib.get('id', '') if nv is not None else ''
            name = nv.find('.//p:cNvPr', NS).attrib.get('name', '') if nv is not None else ''
            tx = "".join(sp.itertext()).strip()

            prst = sp.find('.//a:prstGeom', NS)
            geom = prst.attrib.get('prst', 'custom') if prst is not None else 'custom'

            xfrm = sp.find('.//a:xfrm', NS)
            if xfrm is None:
                continue
            off = xfrm.find('.//a:off', NS)
            ext = xfrm.find('.//a:ext', NS)
            if off is None or ext is None:
                continue

            x, y = int(off.attrib.get('x', 0)), int(off.attrib.get('y', 0))
            w, h = int(ext.attrib.get('cx', 0)), int(ext.attrib.get('cy', 0))

            is_line = (geom == 'line' or w < 1000 or h < 1000 or 'Connector' in name)
            flipH = xfrm.attrib.get('flipH', '0') == '1'
            flipV = xfrm.attrib.get('flipV', '0') == '1'

            if is_line and not tx:
                x1 = x + w if flipH else x
                y1 = y + h if flipV else y
                x2 = x if flipH else x + w
                y2 = y if flipV else y + h
                lines.append({
                    'id': sp_id,
                    'p1': (x1, y1),
                    'p2': (x2, y2),
                    'bbox': (x, y, w, h)
                })
            elif tx:
                # Branch labels (true, false, yes, no)
                if tx.lower() in ('true', 'false', 'yes', 'no', '0', '1') or (len(tx) < 10 and geom == 'rect' and w < 1000000):
                    labels.append({
                        'text': tx,
                        'center': (x + w / 2, y + h / 2)
                    })
                elif not is_template_text(tx) and (geom.startswith('flowChart') or geom in ('rect', 'roundRect', 'diamond', 'ellipse')):
                    nodes.append({
                        'id': f"node_{sp_id}",
                        'text': re.sub(r"\s+", " ", tx).replace('"', "'").strip(),
                        'geom': geom,
                        'bbox': (x, y, w, h)
                    })

        # Only construct graph if we have ≥2 flowchart nodes
        if len(nodes) < 2:
            return [], []

        edges = []
        for line in lines:
            p1, p2 = line['p1'], line['p2']
            closest_start = min(nodes, key=lambda n: dist_to_bbox(p1, n['bbox'])) if nodes else None
            closest_end = min(nodes, key=lambda n: dist_to_bbox(p2, n['bbox'])) if nodes else None

            if closest_start and closest_end and closest_start['id'] != closest_end['id']:
                # Find if any condition label sits near the line midpoint
                line_mid = ((p1[0] + p2[0]) / 2, (p1[1] + p2[1]) / 2)
                edge_label = ""
                for lbl in labels:
                    if math.hypot(lbl['center'][0] - line_mid[0], lbl['center'][1] - line_mid[1]) < 1200000:
                        edge_label = lbl['text']
                        break

                edge_tuple = (closest_start['id'], closest_end['id'], edge_label)
                if edge_tuple not in edges:
                    edges.append(edge_tuple)

        return nodes, edges

    def _handle_figure(self, blob: bytes, slide_num: int, fig_idx: int, fig_dir: Optional[Path]) -> Optional[Dict[str, Any]]:
        """Saves non-branding figure and extracts text labels."""
        from PIL import Image
        try:
            pil_img = Image.open(io.BytesIO(blob))
            w, h = pil_img.size
            if w < 100 or h < 60:
                return None  # Discard small icon slivers

            rel_path = ""
            if fig_dir:
                fname = f"fig_s{slide_num:02d}_{fig_idx:02d}.png"
                target = fig_dir / fname
                pil_img.save(target, format="PNG")
                rel_path = f"figures/{fname}"

            # Fast OCR check
            ocr_text = ""
            try:
                import pytesseract
                ocr_text = pytesseract.image_to_string(pil_img).strip()
            except Exception:
                pass

            ocr_lines = [l.strip() for l in ocr_text.splitlines() if l.strip()]

            return {
                "size": [w, h],
                "path": rel_path,
                "ocr_sample": ocr_text[:200].replace("\n", " "),
                "ocr_lines": ocr_lines
            }
        except Exception:
            return None

    def _render_markdown(
        self, title: str, text_blocks: List[Tuple[int, str]],
        tables: List[List[List[str]]],
        graph_nodes: List[Dict[str, Any]],
        graph_edges: List[Tuple[str, str, str]],
        figures: List[Dict[str, Any]]
    ) -> str:
        """Serializes extracted AST elements into GitHub Flavored Markdown."""
        lines = []

        if title:
            lines.append(f"# {title}\n")

        for indent, txt in text_blocks:
            prefix = "  " * indent + "- "
            lines.append(f"{prefix}{txt}")

        for tbl in tables:
            lines.append("\n| " + " | ".join(tbl[0]) + " |")
            lines.append("| " + " | ".join(["---"] * len(tbl[0])) + " |")
            for r in tbl[1:]:
                padded = r + [""] * (len(tbl[0]) - len(r))
                lines.append("| " + " | ".join(padded[:len(tbl[0])]) + " |")
            lines.append("")

        if graph_nodes:
            lines.append("\n```mermaid\nflowchart TD")
            # Declare nodes
            for n in graph_nodes:
                shape_wrapper = f'["{n["text"]}"]'
                if n["geom"] == "flowChartDecision":
                    shape_wrapper = f'{{"{n["text"]}"}}'
                elif n["geom"] == "roundRect":
                    shape_wrapper = f'("{n["text"]}")'
                lines.append(f'    {n["id"]}{shape_wrapper}')

            # Declare directed edges
            for u, v, lbl in graph_edges:
                lbl_str = f" -- {lbl} --> " if lbl else " --> "
                lines.append(f"    {u}{lbl_str}{v}")
            lines.append("```\n")

        for idx, fig in enumerate(figures, start=1):
            caption = f"Technical Figure ({fig['size'][0]}x{fig['size'][1]} px)"
            lines.append(f"\n> **[Figure {idx}]**: {caption}")
            if fig.get("path"):
                lines.append(f"> ![{caption}]({fig['path']})")
            if fig.get("ocr_sample"):
                lines.append(f"> *Embedded Labels*: {fig['ocr_sample']}")

        return "\n".join(lines).strip()


    def _extract_via_media_bridge(self, pptx_bytes: bytes, out_dir: Optional[Path]) -> List[Dict[str, Any]]:
        """Invokes SlideExtractor in an external/isolated Python environment."""
        import tempfile, json

        if not self.media_python_bin or not (os.path.exists(self.media_python_bin) or shutil.which(self.media_python_bin)):
            raise RuntimeError(
                "Slide extraction requires python-pptx and Pillow. Neither 'pptx' could be imported "
                "in current environment nor was a valid MEDIA_PYTHON_BIN found. "
                "Please run: pip install python-pptx Pillow"
            )

        with tempfile.NamedTemporaryFile(suffix=".pptx", delete=False) as f:
            f.write(pptx_bytes)
            tmp_pptx = f.name

        repo_dir = str(Path(__file__).resolve().parent.parent)
        out_arg = str(out_dir.resolve()) if out_dir else ""

        bridge_code = """import sys, json
from pathlib import Path
repo_dir, tmp_pptx, out_arg = sys.argv[1], sys.argv[2], sys.argv[3]
sys.path.insert(0, repo_dir)
from gulms.slide_extractor import SlideExtractor

with open(tmp_pptx, 'rb') as f:
    b = f.read()
out_d = Path(out_arg) if out_arg else None
extractor = SlideExtractor()
slides = extractor.extract_deck(b, out_dir=out_d)
print('__SLIDES_JSON_START__')
print(json.dumps(slides))
"""
        try:
            res = subprocess.run(
                [self.media_python_bin, "-c", bridge_code, repo_dir, tmp_pptx, out_arg],
                capture_output=True, text=True, check=True
            )
            output = res.stdout
            if "__SLIDES_JSON_START__" in output:
                json_str = output.split("__SLIDES_JSON_START__")[1].strip()
                return json.loads(json_str)
            else:
                raise RuntimeError(f"Media bridge extraction failed: {res.stderr}")
        except subprocess.CalledProcessError as e:
            raise RuntimeError(f"Media bridge extraction failed (exit code {e.returncode}): {e.stderr or e.stdout}")
        finally:
            if os.path.exists(tmp_pptx):
                os.remove(tmp_pptx)
