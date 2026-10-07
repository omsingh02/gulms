"""
GULMS Data Models - Rich Course, Section, and Material abstractions.
Includes PPT auto-summarization, canonical naming, and timestamp tracking for incremental sync.
"""

import os
import sys
import time
import hashlib
from datetime import datetime
from dataclasses import dataclass, field
from typing import List, Dict, Optional, Any
from pathlib import Path
from .categorizer import clean_text, categorize_file, get_acronym, CATEGORY_ICONS
from .downloader import format_size, sanitize_filename, download_file
from .ppt_analyzer import PPTAnalyzer, PPTInfo


@dataclass
class Material:
    """Represents a unique learning material (PDF, PPTX, Doc, Link)."""
    name: str
    filename: str
    filesize: int
    fileurl: Optional[str]
    category: str
    module_type: str
    sections: List[str] = field(default_factory=list)
    timecreated: int = 0
    timemodified: int = 0
    course: Optional[Any] = None
    _client: Optional[Any] = None
    _ppt_info: Optional[Any] = field(default=None, init=False, repr=False)
    _ppt_info_fetched: bool = field(default=False, init=False, repr=False)

    @property
    def size_human(self) -> str:
        return format_size(self.filesize) if self.filesize else "0 B"

    @property
    def icon(self) -> str:
        return CATEGORY_ICONS.get(self.category, "📄")

    @property
    def date_modified(self) -> str:
        if not self.timemodified:
            return "Unknown"
        return datetime.fromtimestamp(self.timemodified).strftime("%Y-%m-%d %H:%M")

    def is_new(self, days: int = 7) -> bool:
        """Returns True if file was created or modified within the last N days."""
        now = time.time()
        cutoff = now - (days * 86400)
        return (self.timemodified or self.timecreated) > cutoff

    @property
    def is_ppt(self) -> bool:
        ext = os.path.splitext(self.filename)[1].lower()
        return ext in (".pptx", ".ppt")

    def get_ppt_info(self) -> Optional[PPTInfo]:
        """Analyze and return PPT content metadata, canonical name, and topic outline."""
        if not self.is_ppt or not self._client or not self.fileurl:
            return None

        if self._ppt_info_fetched:
            return self._ppt_info

        analyzer = PPTAnalyzer()
        cached = analyzer.get_cached(self.fileurl)
        if cached:
            self._ppt_info = cached
            self._ppt_info_fetched = True
            return cached

        # Fetch bytes to analyze
        token_sep = "&token=" if "?" in self.fileurl else "?token="
        token_url = self.fileurl + token_sep + getattr(self._client, "token", "")
        try:
            res = self._client.session.get(token_url)
            if res.status_code == 200:
                key = hashlib.md5(self.fileurl.encode()).hexdigest()
                info = analyzer.analyze_bytes(res.content, raw_filename=self.filename, cache_key=key)
                self._ppt_info = info
                self._ppt_info_fetched = True
                return info
            else:
                print(f"[WARN] Failed to fetch {self.filename} for analysis: HTTP {res.status_code}", file=sys.stderr)
        except Exception as e:
            print(f"[WARN] Error fetching {self.filename} for PPT analysis: {e}", file=sys.stderr)

        self._ppt_info = None
        self._ppt_info_fetched = True
        return None

    @property
    def canonical_name(self) -> str:
        """Returns canonical lecture filename for PPTs or standard clean filename."""
        if self.is_ppt:
            info = self.get_ppt_info()
            if info and info.canonical_filename:
                return info.canonical_filename
        return self.filename

    def export_notes(self, dest_dir: str = "./notes", as_pdf: bool = True, open_viewer: bool = False) -> Optional[Path]:
        """Synthesizes clean study Markdown notes from this presentation."""
        if not self.is_ppt or not self._client or not self.fileurl:
            return None

        from .slide_extractor import SlideExtractor

        target_dir = Path(dest_dir)
        if self.course:
            target_dir = target_dir / sanitize_filename(self.course.shortname or self.course.name)
        target_dir.mkdir(parents=True, exist_ok=True)

        # Download presentation into memory
        token_sep = "&token=" if "?" in self.fileurl else "?token="
        token_url = self.fileurl + token_sep + self._client.token
        res = self._client.session.get(token_url)
        if res.status_code != 200:
            raise RuntimeError(f"Failed to download PPTX bytes: HTTP {res.status_code}")

        deck_bytes = res.content
        extractor = SlideExtractor()
        slides = extractor.extract_deck(deck_bytes, out_dir=target_dir)

        # Build combined Markdown document
        info = self.get_ppt_info()
        base_title = info.title if info else Path(self.canonical_name).stem.replace('_', ' ')
        lec_str = f"Lecture {info.lecture_num}: " if (info and info.lecture_num) else ""
        out_lines = [
            f"# {lec_str}{base_title}\n",
            f"> Source: `{self.filename}` ({len(slides)} slides)\n",
            "---\n"
        ]
        for s in slides:
            title_suffix = f": {s['title']}" if s.get("title") else ""
            out_lines.append(f"## Slide {s['slide_num']}{title_suffix}\n")
            md = s["markdown"]
            if s.get("title") and md.startswith(f"# {s['title']}"):
                md = md[len(f"# {s['title']}"):].lstrip()
            out_lines.append(md)
            out_lines.append("\n---\n")

        out_file = target_dir / f"{Path(self.canonical_name).stem}.md"
        with open(out_file, "w", encoding="utf-8") as f:
            f.write("\n".join(out_lines))

        if as_pdf:
            try:
                from .pdf_renderer import render_markdown_to_pdf, open_in_zathura
                pdf_file = out_file.with_suffix(".pdf")
                render_markdown_to_pdf(
                    "\n".join(out_lines),
                    pdf_file,
                    title=f"{lec_str}{base_title}",
                    base_dir=target_dir
                )
                if open_viewer:
                    open_in_zathura(pdf_file)
                return pdf_file
            except Exception as e:
                # If chromium PDF fails, fallback to markdown file
                print(f"[WARN] PDF generation failed ({e}), saved markdown notes: {out_file}", file=sys.stderr)

        return out_file

    def download(self, dest_dir: str = "./downloads", show_progress: bool = True, use_canonical_name: bool = True) -> Path:
        """Download this material to destination directory."""
        if not self._client or not self.fileurl:
            raise RuntimeError("Cannot download: Missing client session or file URL")
        
        target_dir = Path(dest_dir)
        if self.course:
            target_dir = target_dir / sanitize_filename(self.course.shortname or self.course.name) / sanitize_filename(self.category)

        target_name = self.canonical_name if (use_canonical_name and self.is_ppt) else self.filename
        token_sep = "&token=" if "?" in self.fileurl else "?token="
        download_url = self.fileurl + token_sep + self._client.token

        return download_file(
            url=download_url,
            dest_dir=target_dir,
            filename=target_name,
            expected_size=self.filesize,
            session=self._client.session,
            show_progress=show_progress
        )

    def __repr__(self) -> str:
        return f"<Material [{self.category}] '{self.canonical_name}' ({self.size_human})>"


@dataclass
class Section:
    """Represents a Moodle course section/week."""
    id: int
    name: str
    materials: List[Material] = field(default_factory=list)

    @property
    def total_size(self) -> int:
        return sum(m.filesize for m in self.materials)


class Course:
    """Represents an enrolled course with intelligent caching and deduplication."""
    def __init__(self, raw_data: dict, client: Optional[Any] = None):
        self._raw = raw_data
        self._client = client
        self.id = raw_data["id"]
        self.fullname = clean_text(raw_data.get("fullname", "Unknown Course"))
        self.name = self.fullname
        self.shortname = clean_text(raw_data.get("shortname", ""))
        self.acronym = get_acronym(self.fullname)
        self.timemodified = raw_data.get("timemodified", 0)
        
        self.sections: List[Section] = []
        self._materials_cache: Optional[List[Material]] = None
        self._parse_contents(raw_data.get("sections", []))

    def _parse_contents(self, raw_sections: list):
        seen_materials: Dict[tuple, Material] = {}

        for s_data in raw_sections:
            sec_name = clean_text(s_data.get("name", "General"))
            sec_id = s_data.get("id", 0)
            sec_materials = []

            for mod in s_data.get("modules", []):
                mod_name = clean_text(mod.get("name", ""))
                mod_type = mod.get("modname", "")
                contents = mod.get("contents", [])

                # Purge empty quiz/activity modules
                if not contents:
                    continue

                for f in contents:
                    fname = clean_text(f.get("filename") or mod_name)
                    fsize = f.get("filesize", 0)
                    t_mod = f.get("timemodified", 0)
                    t_cre = f.get("timecreated", 0)
                    key = (fname, fsize)

                    if key in seen_materials:
                        mat = seen_materials[key]
                        if sec_name not in mat.sections:
                            mat.sections.append(sec_name)
                        sec_materials.append(mat)
                    else:
                        cat = categorize_file(fname, fsize, mod_type)
                        mat = Material(
                            name=mod_name or fname,
                            filename=fname,
                            filesize=fsize,
                            fileurl=f.get("fileurl"),
                            category=cat,
                            module_type=mod_type,
                            sections=[sec_name],
                            timecreated=t_cre,
                            timemodified=t_mod,
                            course=self,
                            _client=self._client
                        )
                        seen_materials[key] = mat
                        sec_materials.append(mat)

            if sec_materials:
                self.sections.append(Section(id=sec_id, name=sec_name, materials=sec_materials))

        self._materials_cache = list(seen_materials.values())

    @property
    def materials(self) -> List[Material]:
        """All unique deduplicated materials in this course."""
        return self._materials_cache or []

    @property
    def categories(self) -> Dict[str, List[Material]]:
        """Materials grouped by category."""
        grouped: Dict[str, List[Material]] = {}
        for m in self.materials:
            grouped.setdefault(m.category, []).append(m)
        return grouped

    @property
    def textbooks(self) -> List[Material]:
        """All textbooks, reference books, and official course packs."""
        return self.categories.get("Textbooks & Course Packs", [])

    @property
    def slides(self) -> List[Material]:
        """All lecture presentations (PPTX / PPT)."""
        return self.categories.get("Lecture Slides (PPT)", [])

    @property
    def syllabus(self) -> List[Material]:
        """All syllabi and session plans."""
        return self.categories.get("Syllabus & Session Plans", [])

    @property
    def notes(self) -> List[Material]:
        """All lecture notes, lab manuals, and PDFs."""
        return self.categories.get("Lecture Notes & Documents", [])

    @property
    def total_size(self) -> int:
        return sum(m.filesize for m in self.materials)

    @property
    def total_size_human(self) -> str:
        return format_size(self.total_size)

    def search(self, query: str) -> List[Material]:
        """Search materials within this course."""
        q = query.lower()
        return [m for m in self.materials if q in m.filename.lower() or q in m.name.lower()]

    def download_all(self, dest_dir: str = "./downloads", show_progress: bool = True, use_canonical_name: bool = True) -> List[Path]:
        """Download all unique materials in this course into categorized subfolders."""
        downloaded = []
        for m in self.materials:
            if m.fileurl:
                p = m.download(dest_dir=dest_dir, show_progress=show_progress, use_canonical_name=use_canonical_name)
                downloaded.append(p)
        return downloaded

    def __repr__(self) -> str:
        return f"<Course [{self.acronym or self.shortname}] '{self.fullname}' ({len(self.materials)} materials, {self.total_size_human})>"
