# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-08

First release of `gulms` as a single, self-contained binary. It replaces the earlier Python
tool and the first Rust CLI.

### Added
- Guided first-run `setup`, plus `login` and `logout`. The password is used once to obtain a
  token and is never stored; the token is saved owner-only.
- Incremental `sync` of courses and materials, with a progress bar.
- Lecture analysis (`analyze`): reads each slide deck to find its lecture number, title and
  agenda, so duplicate uploads collapse into one entry per lecture.
- `export`: turns lecture slides into study notes as Markdown and a PDF with nested bullets,
  tables, figures and Mermaid flowcharts. PDFs are produced with any installed Chrome,
  Chromium, Edge or Brave.
- `download-course`: one copy of each lecture, named `Lec-03 - Title.pptx`, plus all other files.
- `slides --outline`, `slides --all`, `view --by-section`, `search --download`, `recent --dest`,
  `--refresh` on `courses`, `view`, `slides` and `download-course`.
- `doctor`: checks sign-in, portal, folders and optional tools, and says how to fix problems.
- `completions` for bash, zsh, fish, PowerShell and elvish.
- Interactive menus for browsing, searching, exporting and downloading.
- Prebuilt binaries for Linux, macOS and Windows, with checksummed install scripts.

### Changed
- `gulms notes` now lists a course's documents. The old Python "slides to study notes"
  command is `gulms export`.
- Study notes are written to `<downloads>/<COURSE>/Study Notes/` rather than `./notes`.
- The Python library API was removed. Existing `config.json` and `ppt_meta.json` keep working.

### Fixed
Compared with the Python implementation, on 194 real lecture decks:
- Flowcharts are only drawn when arrows connect the boxes (previously almost every slide got a
  diagram of unconnected boxes); PowerPoint connectors are now understood.
- Pictures placed in content placeholders are no longer dropped.
- Soft line breaks no longer glue words together.
- `<`, `>` and `&` in lecture text no longer break the PDF.
- Markdown tables no longer show their `---` rule as a data row, and nested bullets stay nested.
- Decks that crashed the Python extractor now convert.

[Unreleased]: https://github.com/omsingh02/gulms/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/omsingh02/gulms/releases/tag/v0.1.0
