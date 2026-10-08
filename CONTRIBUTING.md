# Contributing to gulms

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

## Reporting a problem

Run `gulms doctor` first; it catches most setup problems. If that doesn't help, open an
[issue](https://github.com/omsingh02/gulms/issues/new/choose) and paste its output.
**Never paste your token, password, or a `config.json`.**

## Development setup

You need a [Rust toolchain](https://rustup.rs) (1.88 or newer).

```bash
git clone https://github.com/omsingh02/gulms && cd gulms
cargo run -- --help          # run from source
cargo test                   # unit tests + end-to-end tests against a fake Moodle server
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The end-to-end tests (`tests/e2e.rs`) start a fake portal on localhost and run the real binary
against it, so nothing touches a real LMS. The PDF test needs Chrome, Chromium, Edge or Brave and
is skipped without one.

Use `GULMS_CONFIG_DIR` and `GULMS_CACHE_DIR` to keep experiments away from your real settings.

## Layout

| Path | What lives there |
| :--- | :--- |
| `src/cli.rs`, `src/main.rs` | Commands and their handlers |
| `src/interactive.rs` | Menus |
| `src/auth.rs`, `src/sync.rs`, `src/downloader.rs` | Sign-in and talking to the portal |
| `src/analyzer.rs` | Reads a deck to find its lecture number and title |
| `src/pptx.rs`, `src/slide_extractor.rs` | Reads slides into Markdown |
| `src/render.rs` | Markdown to HTML to PDF |
| `src/lectures.rs` | Analysis, export and course download built on the above |
| `tests/fixtures` | Small synthetic decks and their expected output |

## Pull requests

- Keep each PR focused, and add a test for behaviour changes.
- Make sure `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass;
  CI runs them on Linux, macOS and Windows.
- Add a line to `CHANGELOG.md` under "Unreleased" for anything users will notice.
- **Do not commit real course material, tokens, or personal data.** Fixtures must be synthetic.

## Reading slides

`ExtractOptions::default()` reproduces the original Python extractor exactly; `recommended()` turns
on the fixes. If you change extraction, check both modes, and prefer adding a fix behind a named
option over silently changing the faithful behaviour.

## License

By contributing you agree that your work is licensed under the project's terms:
MIT OR Apache-2.0.
