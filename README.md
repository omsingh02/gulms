# GULMS - Galgotias University LMS Python Library & CLI

A fast, modular Python library and terminal client for Galgotias University LMS (`gulms.galgotiasuniversity.org`), powered by Moodle's native Mobile REST API.

- **Purges 60%+ LMS clutter**: Silently removes all 2,740+ empty quiz activities, Wooclap, Wooflash, and empty placeholder sections.
- **PPT Auto-Summarization & Canonical Naming**: Parses `.pptx` XML to extract real lecture numbers, titles, and slide outlines instead of random numbers/names (e.g. saves as `Lec-02 - Register Transfer Language (RTL).pptx`).
- **Unique Content Identification**: Generates SHA-256 fingerprints of non-boilerplate slide text to identify identical or duplicate slide decks across different professors.
- **Course Selection**: Pick and save your active semester courses once to permanently hide past semester courses.
- **Modular Python API**: Import `GULMSClient`, `Course`, and `Material` directly in your Python scripts.

---

## Installation & Setup

### Install via pip

```bash
# Core package (CLI + basic slide extraction)
pip install .

# With optional PDF outline bookmarks and OCR capabilities
pip install ".[all]"
```

### System Prerequisites

To use slide-to-PDF note rendering and automatic document viewing, ensure the following system tools are available:

- **Headless Browser (PDF Printing)**: `chromium` or `google-chrome` (used to generate crisp vector PDFs).
- **Document Viewer**: `zathura` (recommended for fast keyboard navigation) or any standard viewer via `xdg-open`.
- **OCR Engine (Optional)**: `tesseract` (if extracting embedded image text from slide diagrams).

### Environment Variables

| Variable | Description | Default |
| :--- | :--- | :--- |
| `CHROME_BIN` | Custom path to Chromium / Google Chrome executable | Auto-detected from PATH |
| `MEDIA_PYTHON_BIN` | Custom Python interpreter path containing media libraries | `~/.local/bin/media-python` or `python3` |
| `BENCHMARK_DIR` | Directory containing benchmark test presentation decks | `tests/fixtures/` |

---

### First-Time Setup
Running `gulms` for the first time opens an interactive guided setup:
- Confirms your student profile
- Lets you select your current semester courses (e.g. `2, 10, 11-16`)
- Sets your preferred download directory
- Saves your credentials safely in `~/.config/gulms/config.json` (outside the code repository)

---

## CLI Usage

```bash
# Interactive guided menu
gulms

# List your saved active courses
gulms courses

# Select / reconfigure active courses anytime
gulms select

# View organized course contents (COA, DBMS, DSA...)
gulms view COA

# Auto-summarize & list slides in chronological order (Lec-00, Lec-01...)
gulms slides COA

# List all slides without deduplication / grouping
gulms slides COA -a

# Detailed outline and topic summary for any lecture
gulms summarize "Lesson_2"

# Download course with auto-renamed canonical slides
gulms download-course COA
```

---

## Python Library Usage

```python
from gulms import GULMSClient

client = GULMSClient()

# Get course by acronym
coa = client.get_course("COA")

# Inspect and download slides with canonical names
for slide in coa.slides[:5]:
    # Analyzes actual XML structure of the presentation
    info = slide.get_ppt_info()
    print(f"{info.canonical_filename} (Hash: {info.content_hash})")
    print("Topics:", info.topics)

    # Downloads automatically as 'Lec-02 - Register Transfer Language (RTL).pptx'
    slide.download("./my_lectures", use_canonical_name=True)
```
