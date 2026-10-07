# gulms-cli

Blazing fast, interactive inline CLI written in Rust for Galgotias University LMS (`gulms`).

## Features

- **Interactive Inline UI**: Powered by `inquire` for intuitive fuzzy searching, menu navigation, multi-select course configuration, and arrow-key selection.
- **Incremental Delta Sync**: Synchronizes course updates in milliseconds by checking remote timestamps, downloading only new and modified courses.
- **Canonical Slide Deduplication**: Reconstructs lecture timelines from PPT/PDF slides across modules and weeks, deduplicating redundant uploads and tracking slide counts.
- **Smart Categorization**: Organizes course content into *Syllabus*, *Lecture Slides*, *Notes*, *Textbooks*, *Code & Archives*, and *Web Links*.
- **Integrated Downloader & Viewers**: Streaming HTTP downloader with progress bar; opens PDFs directly in `zathura` or system viewers, with clipboard token copying (`wl-copy`/`xclip`).

## Installation

```bash
cargo build --release
cp target/release/gulms-cli ~/.local/bin/
```

## Usage

### Interactive Mode (Default)
Run without arguments to launch the full interactive inline experience:
```bash
gulms-cli
```

### CLI Commands
```bash
# Display account info and sync status
gulms-cli whoami

# List enrolled and tracked courses
gulms-cli courses
gulms-cli courses --all

# View deduplicated slides for a course (e.g. DBMS, COA)
gulms-cli slides DBMS

# View lecture notes and documents
gulms-cli notes DBMS

# Search materials globally across all courses
gulms-cli search "normalization"

# View recent uploads from the past 7 days
gulms-cli recent --days 7

# Run incremental Moodle sync
gulms-cli sync
gulms-cli sync --force
```
