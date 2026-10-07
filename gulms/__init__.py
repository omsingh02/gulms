"""
GULMS - Python Library for Galgotias University LMS (Moodle REST API).
Fast, clutter-free material navigation, course selection, and PPT auto-summarization.
"""

from .client import GULMSClient
from .models import Course, Section, Material
from .categorizer import clean_text, categorize_file, get_acronym, CATEGORY_ICONS
from .downloader import download_file, format_size
from .ppt_analyzer import PPTAnalyzer, PPTInfo
from .slide_extractor import SlideExtractor

__all__ = [
    "GULMSClient",
    "Course",
    "Section",
    "Material",
    "clean_text",
    "categorize_file",
    "get_acronym",
    "CATEGORY_ICONS",
    "download_file",
    "format_size",
    "PPTAnalyzer",
    "PPTInfo",
    "SlideExtractor",
]

__version__ = "1.2.0"
