"""
GULMS Categorizer - Cleans HTML noise, purges empty modules, and classifies materials.
"""

import os
import re
import html

CATEGORY_ICONS = {
    "Syllabus & Session Plans": "📋",
    "Textbooks & Course Packs": "📚",
    "Lecture Slides (PPT)": "📑",
    "Lecture Notes & Documents": "📝",
    "Code & Archives": "📦",
    "Web Links & Videos": "🔗",
    "Other Materials": "📄"
}

def clean_text(text: str) -> str:
    """Strip raw HTML tags and unescape HTML entities."""
    if not text:
        return ""
    text = html.unescape(text)
    text = re.sub(r'<[^>]+>', '', text)
    return text.strip()

def get_acronym(course_name: str) -> str:
    """Generate clean acronym from course name (e.g. COA, DBMS, DSA)."""
    # Strip parenthetical codes like (R1UC305T)
    cleaned = re.sub(r'\(.*?\)', '', clean_text(course_name))
    words = re.findall(r'[A-Za-z0-9]+', cleaned)
    stop_words = {"and", "with", "using", "of", "the", "for", "in", "to"}
    return "".join(w[0].upper() for w in words if w.lower() not in stop_words)

def categorize_file(fname: str, fsize: int, mtype: str = "resource") -> str:
    """Categorize file based on extension, size, and naming cues."""
    fname_clean = clean_text(fname)
    lower = fname_clean.lower()
    ext = os.path.splitext(fname_clean)[1].lower()
    
    if ext in [".xls", ".xlsx"] or "session plan" in lower or "syllabus" in lower:
        return "Syllabus & Session Plans"
    if "book" in lower or "course pack" in lower or "coursepack" in lower or "textbook" in lower or (ext == ".pdf" and fsize > 6*1024*1024):
        return "Textbooks & Course Packs"
    if ext in [".pptx", ".ppt", ".pps", ".ppsx"]:
        return "Lecture Slides (PPT)"
    if ext in [".zip", ".rar", ".tar", ".gz", ".7z", ".java", ".py", ".c", ".cpp", ".sql"]:
        return "Code & Archives"
    if ext in [".doc", ".docx", ".pdf", ".txt", ".odt"]:
        return "Lecture Notes & Documents"
    if mtype == "url":
        return "Web Links & Videos"
    return "Other Materials"
