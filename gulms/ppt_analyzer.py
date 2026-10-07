"""
GULMS PPT Analyzer - Auto-summarization, unique identification, and canonical naming of slides.
Parses OpenXML (.pptx) structures to extract real lecture titles, session numbers, and agenda topics.
"""

import os
import re
import io
import json
import hashlib
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import List, Dict, Optional, Union
from dataclasses import dataclass, field

CACHE_DIR = Path.home() / ".cache" / "gulms"
CACHE_DIR.mkdir(parents=True, exist_ok=True)
PPT_META_CACHE = CACHE_DIR / "ppt_meta.json"

BOILERPLATE_PHRASES = {
    "galgotias university", "school of computer science", "program name:", "b.tech",
    "course code:", "course name:", "prepared by", "vision & mission", "vision statement:",
    "mission statement:", "program educational objectives", "peo", "program outcomes",
    "pos", "course outcomes", "cos", "program specific outcomes", "psos",
    "‹#›", "m1:", "m2:", "m3:", "m4:", "reflect on the responses", "how these machines work",
    "at the end of this session", "learning outcomes:", "learning outcome"
}


@dataclass
class PPTInfo:
    """Metadata extracted from PPT content."""
    lecture_num: Optional[int]
    title: str
    canonical_filename: str
    topics: List[str] = field(default_factory=list)
    slide_count: int = 0
    content_hash: str = ""
    raw_filename: str = ""

    @property
    def summary_text(self) -> str:
        num_str = f"Lecture {self.lecture_num:02d}" if self.lecture_num is not None else "Lecture (Unnumbered)"
        lines = [
            f"📑 {num_str}: {self.title}",
            f"   Slides: {self.slide_count} | Hash: {self.content_hash}",
        ]
        if self.topics:
            lines.append("   Key Topics Covered:")
            for t in self.topics[:6]:
                lines.append(f"    • {t}")
        return "\n".join(lines)

    def to_dict(self) -> dict:
        return {
            "lecture_num": self.lecture_num,
            "title": self.title,
            "canonical_filename": self.canonical_filename,
            "topics": self.topics,
            "slide_count": self.slide_count,
            "content_hash": self.content_hash,
            "raw_filename": self.raw_filename
        }

    @classmethod
    def from_dict(cls, data: dict) -> "PPTInfo":
        return cls(
            lecture_num=data.get("lecture_num"),
            title=data.get("title", "Lecture"),
            canonical_filename=data.get("canonical_filename", "Lecture.pptx"),
            topics=data.get("topics", []),
            slide_count=data.get("slide_count", 0),
            content_hash=data.get("content_hash", ""),
            raw_filename=data.get("raw_filename", "")
        )


class PPTAnalyzer:
    """Analyzes and caches PPT summaries and canonical names."""
    def __init__(self, cache_file: Path = PPT_META_CACHE):
        self.cache_file = Path(cache_file)
        self.cache = self._load_cache()

    def _load_cache(self) -> dict:
        if self.cache_file.exists():
            try:
                with open(self.cache_file, "r", encoding="utf-8") as f:
                    return json.load(f)
            except (json.JSONDecodeError, OSError) as e:
                import sys
                print(f"[WARN] Failed to load PPT metadata cache from {self.cache_file}: {e}", file=sys.stderr)
        return {}

    def _save_cache(self):
        with open(self.cache_file, "w", encoding="utf-8") as f:
            json.dump(self.cache, f, indent=2)

    def get_cached(self, file_id_or_url: str) -> Optional[PPTInfo]:
        key = hashlib.md5(file_id_or_url.encode()).hexdigest()
        if key in self.cache:
            return PPTInfo.from_dict(self.cache[key])
        return None

    def analyze_bytes(self, file_bytes: bytes, raw_filename: str = "", cache_key: str = None) -> PPTInfo:
        """Parse .pptx OpenXML structure in memory."""
        try:
            z = zipfile.ZipFile(io.BytesIO(file_bytes))
        except Exception as e:
            # Not a valid zip/pptx (e.g. legacy binary .ppt)
            return self._fallback_info(raw_filename)

        slide_names = [n for n in z.namelist() if re.match(r"ppt/slides/slide\d+\.xml", n)]
        slide_names.sort(key=lambda x: int(re.search(r"\d+", x).group()))

        all_slides_clean = []
        outline_topics = []

        for sname in slide_names:
            try:
                tree = ET.fromstring(z.read(sname))
                texts = [elem.text.strip() for elem in tree.iter() if elem.text and elem.text.strip()]
                # filter boilerplates
                clean = [t for t in texts if not any(bp in t.lower() for bp in BOILERPLATE_PHRASES)]
                if clean:
                    all_slides_clean.append(clean)
                
                # Check for Outline / Agenda slides
                if any("outline" in t.lower() or "agenda" in t.lower() or "objective" in t.lower() for t in texts):
                    # extract bullet points on this slide
                    for t in clean:
                        if len(t) > 3 and not re.match(r"^(?:session|agenda|outline|objectives?)\b", t, re.I):
                            if t not in outline_topics:
                                outline_topics.append(t)
            except Exception:
                continue

        # 1. Detect Lecture Number
        full_intro = " ".join(" ".join(st) for st in all_slides_clean[:5])
        lec_match = re.search(r"(?:session|lecture|lec|lesson)\s*(?:no\.?)?\s*[:\-\[]?\s*(\d+)", full_intro, re.I)
        if not lec_match:
            # Try raw filename
            lec_match = re.search(r"(?:session|lecture|lec|lesson|l)\s*[-_]?\s*(\d+)", raw_filename, re.I)
        lec_num = int(lec_match.group(1)) if lec_match else None

        # 2. Detect Main Topic Title
        title_candidates = []
        if all_slides_clean:
            for t in all_slides_clean[0]:
                t_low = t.lower()
                if any(x in t_low for x in ["course name", "program name", "prepared by", "b.tech", "galgotias", "instructor name", "students-centred"]):
                    continue
                if re.match(r"^(?:session|lecture|lec|lesson)\s*(?:no\.?)?\s*[:\-\[]?\s*\d+", t, re.I):
                    continue
                if len(t) >= 2:
                    title_candidates.append(t)

        main_title = ""
        if title_candidates:
            main_title = title_candidates[0]
            # Connect dangling conjunctions/prepositions (e.g. "Database System Vs" -> "Database System Vs File System")
            idx = 1
            while idx < len(title_candidates) and (re.search(r"\b(?:vs|vs\.|and|or|of|to|in|for|with|the|&)\s*$", main_title, re.I) or len(main_title) < 10):
                next_part = title_candidates[idx]
                if any(x in next_part.lower() for x in ["instructor", "session", "course", "unit", "module", "galgotias"]):
                    break
                clean_next = re.split(r"[,;]", next_part)[0].strip()
                if clean_next:
                    main_title = f"{main_title} {clean_next}".strip()
                idx += 1
        elif outline_topics:
            main_title = outline_topics[0]
        else:
            # Clean from raw filename
            main_title = re.sub(r'^(?:lesson|lecture|lec|session)[-_ ]*\d+[-_ ]*', '', Path(raw_filename).stem, flags=re.I)
            main_title = re.sub(r'\(.*?\)', '', main_title).strip()

        # Sanitize title
        main_title = re.sub(r'[\\/*?:"<>|]', " ", main_title).strip()
        main_title = re.sub(r'\s+', ' ', main_title)
        main_title = re.sub(r"[\s,;:\-]+$", "", main_title)
        main_title = re.sub(r"\b(?:vs|vs\.|and|or|of|to|for|with|in|the|&)\s*$", "", main_title, flags=re.I).strip()
        if not main_title:
            main_title = "Lecture Material"

        # 3. Canonical Filename
        num_prefix = f"Lec-{lec_num:02d}" if lec_num is not None else "Lec-??"
        # Cap title length to 50 chars for clean filenames
        short_title = main_title[:50].strip()
        canonical_filename = f"{num_prefix} - {short_title}.pptx"

        # 4. Content Hash (Unique content identification)
        normalized_content = " ".join(" ".join(st) for st in all_slides_clean)
        content_hash = hashlib.sha256(normalized_content.encode("utf-8")).hexdigest()[:12]

        info = PPTInfo(
            lecture_num=lec_num,
            title=main_title,
            canonical_filename=canonical_filename,
            topics=outline_topics[:8],
            slide_count=len(slide_names),
            content_hash=content_hash,
            raw_filename=raw_filename
        )

        if cache_key:
            self.cache[cache_key] = info.to_dict()
            self._save_cache()

        return info

    def _fallback_info(self, raw_filename: str) -> PPTInfo:
        stem = Path(raw_filename).stem
        lec_match = re.search(r"(?:session|lecture|lec|lesson|l)\s*[-_]?\s*(\d+)", stem, re.I)
        lec_num = int(lec_match.group(1)) if lec_match else None
        clean_title = re.sub(r'^(?:lesson|lecture|lec|session)[-_ ]*\d+[-_ ]*', '', stem, flags=re.I).strip()
        num_prefix = f"Lec-{lec_num:02d}" if lec_num is not None else "Lec-??"
        canonical = f"{num_prefix} - {clean_title}.pptx"
        return PPTInfo(
            lecture_num=lec_num,
            title=clean_title or "Lecture",
            canonical_filename=canonical,
            topics=[],
            slide_count=0,
            content_hash=hashlib.sha256(raw_filename.encode()).hexdigest()[:12],
            raw_filename=raw_filename
        )
