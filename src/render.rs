//! Study notes: Markdown -> standalone styled HTML -> vector PDF through a
//! headless Chromium-family browser (Chrome, Chromium, Edge, Brave).
//!
//! Ported from the Python renderer. Deliberate changes: text is HTML-escaped (the
//! original swallowed `<stdio.h>`), nested bullets nest, a table at the very end of
//! a document is kept, and the Mermaid script is pinned and only loaded when a
//! diagram is present. No fonts are fetched; system fonts are used.

use base64::Engine;
use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const MERMAID_SRC: &str = "https://cdn.jsdelivr.net/npm/mermaid@10.9.8/dist/mermaid.min.js";
const MERMAID_SRI: &str = "sha384-N3QqR/7q+xm3BGX+CBbNI8AUmRRqcsDzToy+0z1NLDI0QmTKW8zvwLvqulJgk3dP";

static RE_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[(.*?)\]\((.*?)\)").unwrap());
static RE_BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.*?)\*\*").unwrap());
static RE_ITALIC: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r"(?<!\*)\*(?!\*)(.*?)(?<!\*)\*(?!\*)").unwrap());
static RE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
// A Markdown table's `| --- | :-: |` rule row. (The original pattern lacked the inner `|`,
// so it only matched one-column tables and showed the rule as a data row.)
static RE_TABLE_RULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\|[\s\-:|]+\|$").unwrap());
static RE_BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)-\s+").unwrap());

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn mime_for(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// Inline Markdown: images (inlined as data URIs), bold, italics, code.
fn format_inline(text: &str, base_dir: Option<&Path>) -> String {
    let text = escape_html(text);

    let text = RE_IMAGE.replace_all(&text, |caps: &regex::Captures| {
        let alt = &caps[1];
        let src = &caps[2];
        let mut final_src = src.to_string();
        let remote =
            src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:");
        if let (Some(base), false) = (base_dir, remote) {
            let path = base.join(src);
            if let Ok(bytes) = std::fs::read(&path) {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
                final_src = format!(
                    "data:{};base64,{}",
                    mime_for(ext),
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                );
            }
        }
        let caption = if alt.is_empty() {
            String::new()
        } else {
            format!("<figcaption>{alt}</figcaption>")
        };
        format!(
            r#"<figure class="doc-figure"><img src="{final_src}" alt="{alt}">{caption}</figure>"#
        )
    });
    let text = RE_BOLD.replace_all(&text, "<strong>$1</strong>");
    let text = RE_ITALIC.replace_all(&text, "<em>$1</em>");
    RE_CODE.replace_all(&text, "<code>$1</code>").into_owned()
}

/// Wrap a "[Figure N]" caption and the picture right after it in one block, so a page
/// break can never leave the caption behind on one page and the picture on the next.
fn group_figure_captions(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut iter = lines.into_iter().peekable();
    while let Some(line) = iter.next() {
        let is_caption = line.starts_with("<blockquote><strong>[Figure ");
        let next_is_picture = iter
            .peek()
            .is_some_and(|n| n.starts_with(r#"<figure class="doc-figure">"#));
        if is_caption && next_is_picture {
            let picture = iter.next().unwrap_or_default();
            out.push(format!(r#"<div class="figure-card">{line}{picture}</div>"#));
        } else {
            out.push(line);
        }
    }
    out
}

fn close_lists(html: &mut Vec<String>, depth: &mut usize) {
    while *depth > 0 {
        html.push("</ul>".to_string());
        *depth -= 1;
    }
}

fn flush_table(html: &mut Vec<String>, table_buf: &mut Vec<String>, base_dir: Option<&Path>) {
    html.push("<table>".to_string());
    let rows: Vec<Vec<&str>> = table_buf
        .iter()
        .filter(|r| !RE_TABLE_RULE.is_match(r))
        .map(|r| r.trim_matches('|').split('|').collect())
        .collect();
    if let Some((head, body)) = rows.split_first() {
        let cells = |row: &[&str], tag: &str| -> String {
            row.iter()
                .map(|c| format!("<{tag}>{}</{tag}>", format_inline(c.trim(), base_dir)))
                .collect()
        };
        html.push(format!("  <thead><tr>{}</tr></thead>", cells(head, "th")));
        html.push("  <tbody>".to_string());
        for r in body {
            html.push(format!("    <tr>{}</tr>", cells(r, "td")));
        }
        html.push("  </tbody>".to_string());
    }
    html.push("</table>".to_string());
    table_buf.clear();
}

pub fn markdown_to_html(md_text: &str, title: &str, base_dir: Option<&Path>) -> String {
    let mut html: Vec<String> = Vec::new();
    let (mut in_code, mut code_lang, mut code_buf) = (false, String::new(), Vec::<&str>::new());
    let (mut in_mermaid, mut mermaid_buf) = (false, Vec::<&str>::new());
    let (mut in_table, mut table_buf) = (false, Vec::<String>::new());
    // Indentation (in spaces) of each currently open <ul>.
    let mut list_indents: Vec<usize> = Vec::new();
    let mut list_depth = 0usize;
    let mut has_mermaid = false;

    for line in md_text.lines() {
        // Code fences
        if line.starts_with("```") {
            if in_code {
                in_code = false;
                html.push(format!(
                    r#"<pre><code class="language-{}">{}</code></pre>"#,
                    code_lang,
                    escape_html(&code_buf.join("\n"))
                ));
                code_buf.clear();
            } else if in_mermaid {
                in_mermaid = false;
                has_mermaid = true;
                html.push(format!(
                    "<div class=\"mermaid\">\n{}\n</div>",
                    escape_html(&mermaid_buf.join("\n"))
                ));
                mermaid_buf.clear();
            } else {
                let tag = line.trim()[3..].trim();
                if tag == "mermaid" {
                    in_mermaid = true;
                } else {
                    in_code = true;
                    code_lang = tag.to_string();
                }
            }
            continue;
        }
        if in_code {
            code_buf.push(line);
            continue;
        }
        if in_mermaid {
            mermaid_buf.push(line);
            continue;
        }

        // Tables
        let stripped = line.trim();
        if stripped.starts_with('|') && stripped.ends_with('|') {
            in_table = true;
            table_buf.push(stripped.to_string());
            continue;
        } else if in_table {
            in_table = false;
            flush_table(&mut html, &mut table_buf, base_dir);
        }

        // Bullet lists (nested by indentation)
        if let Some(caps) = RE_BULLET.captures(line) {
            let indent = caps[1].len();
            let content = format_inline(&line[caps[0].len()..], base_dir);
            while list_indents.last().is_some_and(|last| indent < *last) {
                list_indents.pop();
                html.push("</ul>".to_string());
                list_depth -= 1;
            }
            if list_indents.last().is_none_or(|last| indent > *last) {
                list_indents.push(indent);
                html.push("<ul>".to_string());
                list_depth += 1;
            }
            html.push(format!("  <li>{content}</li>"));
            continue;
        } else if list_depth > 0 && stripped.is_empty() {
            close_lists(&mut html, &mut list_depth);
            list_indents.clear();
            continue;
        }

        let mut leave_list = |html: &mut Vec<String>| {
            close_lists(html, &mut list_depth);
            list_indents.clear();
        };

        // Headings
        if let Some(rest) = line.strip_prefix("# ") {
            leave_list(&mut html);
            html.push(format!("<h1>{}</h1>", format_inline(rest.trim(), base_dir)));
            continue;
        } else if let Some(rest) = line.strip_prefix("## ") {
            leave_list(&mut html);
            html.push(format!("<h2>{}</h2>", format_inline(rest.trim(), base_dir)));
            continue;
        } else if let Some(rest) = line.strip_prefix("### ") {
            leave_list(&mut html);
            html.push(format!("<h3>{}</h3>", format_inline(rest.trim(), base_dir)));
            continue;
        }

        // Blockquotes (an image inside one is shown as a figure)
        if let Some(rest) = line.strip_prefix("> ") {
            leave_list(&mut html);
            let raw = rest.trim();
            if raw.starts_with("![") && raw.ends_with(')') {
                html.push(format_inline(raw, base_dir));
            } else {
                html.push(format!(
                    "<blockquote>{}</blockquote>",
                    format_inline(raw, base_dir)
                ));
            }
            continue;
        }

        if matches!(stripped, "---" | "***") {
            leave_list(&mut html);
            html.push("<hr>".to_string());
            continue;
        }

        if stripped.is_empty() {
            leave_list(&mut html);
            continue;
        }

        if stripped.starts_with("![") && stripped.ends_with(')') {
            leave_list(&mut html);
            html.push(format_inline(stripped, base_dir));
            continue;
        }

        html.push(format!("<p>{}</p>", format_inline(line, base_dir)));
    }

    close_lists(&mut html, &mut list_depth);
    if in_table {
        flush_table(&mut html, &mut table_buf, base_dir);
    }
    let body_html = group_figure_captions(html).join("\n");

    let mermaid = if has_mermaid {
        format!(
            r#"<script src="{MERMAID_SRC}" integrity="{MERMAID_SRI}" crossorigin="anonymous"></script>
<script>
mermaid.initialize({{ startOnLoad: true, theme: 'neutral', securityLevel: 'strict' }});
</script>
"#
        )
    } else {
        String::new()
    };

    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n{}<style>\n{}</style>\n</head>\n<body>\n{}\n</body>\n</html>\n",
        escape_html(title),
        mermaid,
        STYLE,
        body_html
    )
}

const STYLE: &str = r#"@page {
    size: A4;
    margin: 20mm 18mm 20mm 18mm;
    @bottom-right {
        content: counter(page);
        font-family: 'Inter', sans-serif;
        font-size: 8.5pt;
        color: #94a3b8;
    }
}

body {
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    font-size: 10.5pt;
    line-height: 1.65;
    color: #1e293b;
    background-color: #ffffff;
    max-width: 840px;
    margin: 0 auto;
}

h1 {
    font-size: 21pt;
    font-weight: 700;
    color: #0f172a;
    border-bottom: 2px solid #e2e8f0;
    padding-bottom: 8px;
    margin-top: 32px;
    margin-bottom: 16px;
    page-break-after: avoid;
}

h2 {
    font-size: 15pt;
    font-weight: 600;
    color: #1e293b;
    margin-top: 28px;
    margin-bottom: 12px;
    border-bottom: 1px solid #f1f5f9;
    padding-bottom: 4px;
    page-break-after: avoid;
}

h3 {
    font-size: 12pt;
    font-weight: 600;
    color: #334155;
    margin-top: 20px;
    margin-bottom: 8px;
    page-break-after: avoid;
}

p {
    margin-top: 0;
    margin-bottom: 10px;
}

ul {
    padding-left: 22px;
    margin-top: 0;
    margin-bottom: 14px;
}

ul ul {
    margin-bottom: 4px;
}

li {
    margin-bottom: 5px;
}

table {
    width: 100%;
    border-collapse: collapse;
    margin: 18px 0;
    font-size: 9.5pt;
    page-break-inside: avoid;
}

th, td {
    border: 1px solid #cbd5e1;
    padding: 8px 12px;
    text-align: left;
    vertical-align: top;
}

th {
    background-color: #f1f5f9;
    font-weight: 600;
    color: #0f172a;
    border-bottom: 2px solid #94a3b8;
}

tr:nth-child(even) td {
    background-color: #f8fafc;
}

blockquote {
    border-left: 3.5px solid #3b82f6;
    background: #f0f7ff;
    margin: 14px 0;
    padding: 10px 14px;
    border-radius: 0 6px 6px 0;
    color: #1e40af;
    font-size: 10pt;
    page-break-inside: avoid;
}

hr {
    border: none;
    border-top: 1px solid #e2e8f0;
    margin: 28px 0;
}

.mermaid {
    display: flex;
    justify-content: center;
    background: #ffffff;
    border: 1px solid #e2e8f0;
    border-radius: 6px;
    padding: 16px;
    margin: 20px 0;
    page-break-inside: avoid;
}

.mermaid svg {
    width: auto !important;
    max-width: 100%;
    max-height: 700px;
    height: auto;
}

.figure-card {
    page-break-inside: avoid;
    break-inside: avoid;
}

figure.doc-figure {
    margin: 20px auto;
    text-align: center;
    page-break-inside: avoid;
}

figure.doc-figure img {
    max-width: 95%;
    max-height: 480px;
    height: auto;
    border-radius: 6px;
    border: 1px solid #cbd5e1;
    box-shadow: 0 1px 3px rgba(0,0,0,0.05);
    background: #ffffff;
    display: inline-block;
}

figure.doc-figure figcaption {
    font-size: 8.5pt;
    color: #64748b;
    margin-top: 6px;
    font-style: italic;
}

code, pre {
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', Consolas, monospace;
    font-size: 9pt;
}

pre {
    background: #f8fafc;
    color: #0f172a;
    border: 1px solid #e2e8f0;
    padding: 14px 16px;
    border-radius: 6px;
    overflow-x: auto;
    line-height: 1.5;
    page-break-inside: avoid;
}

p code, li code, td code {
    background: #f1f5f9;
    color: #0f172a;
    padding: 2px 5px;
    border-radius: 4px;
    border: 1px solid #e2e8f0;
}
"#;

// ------------------------------------------------------------------ the browser

/// Absolute path of a Chromium-family browser, if one is installed.
/// `CHROME_BIN` (a path or a command name) wins over detection.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("CHROME_BIN").filter(|v| !v.is_empty()) {
        let custom = PathBuf::from(custom);
        if custom.is_file() {
            return Some(custom);
        }
        if let Some(found) = custom.to_str().and_then(find_in_path) {
            return Some(found);
        }
    }

    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "brave",
        "brave-browser",
        "microsoft-edge",
        "microsoft-edge-stable",
        "msedge",
    ] {
        if let Some(found) = find_in_path(name) {
            return Some(found);
        }
    }

    let mut known: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Chromium.app/Contents/MacOS/Chromium",
        ] {
            known.push(Path::new("/Applications").join(app));
        }
    }
    if cfg!(windows) {
        for var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                known.push(base.join(r"Google\Chrome\Application\chrome.exe"));
                known.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
                known.push(base.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"));
            }
        }
    }
    known.into_iter().find(|p| p.is_file())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{name}.exe"));
            return exe.is_file().then_some(exe);
        }
        None
    })
}

fn file_url(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    // Strip the `\\?\` prefix that canonicalize adds on Windows.
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    let encoded: String = s
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            '%' => "%25".to_string(),
            c => c.to_string(),
        })
        .collect();
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// Render Markdown to a PDF at `out_pdf`. Headings become the PDF's outline.
pub fn render_markdown_to_pdf(
    md_text: &str,
    out_pdf: &Path,
    title: &str,
    base_dir: Option<&Path>,
) -> Result<(), String> {
    let browser = find_browser().ok_or_else(|| {
        "no Chrome, Chromium, Edge or Brave found for PDF output (set CHROME_BIN to point at one, \
         or use --no-pdf to keep just the Markdown)"
            .to_string()
    })?;

    if let Some(parent) = out_pdf.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }

    let work = std::env::temp_dir().join(format!(
        "gulms-pdf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&work).map_err(|e| format!("could not create a temp dir: {e}"))?;
    let html_path = work.join("notes.html");
    std::fs::write(&html_path, markdown_to_html(md_text, title, base_dir))
        .map_err(|e| format!("could not write temp file: {e}"))?;

    let log_path = work.join("browser.log");
    let run = |sandbox: bool| -> Result<(), String> {
        let _ = std::fs::remove_file(out_pdf); // never mistake a stale file for a fresh one
        let mut cmd = Command::new(&browser);
        cmd.arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg("--disable-extensions")
            .arg("--disable-breakpad") // no crash-reporter helper left running afterwards
            .arg("--use-mock-keychain") // macOS: never block on a keychain prompt
            .arg("--password-store=basic")
            .arg(format!(
                "--user-data-dir={}",
                work.join("profile").display()
            ))
            .arg("--virtual-time-budget=4000")
            .arg("--no-pdf-header-footer")
            .arg("--generate-pdf-document-outline");
        if !sandbox {
            cmd.arg("--no-sandbox");
        }
        cmd.arg(format!("--print-to-pdf={}", out_pdf.display()))
            .arg(file_url(&html_path));
        run_browser(&mut cmd, out_pdf, &log_path, BROWSER_TIMEOUT)
            .map_err(|e| format!("{}: {e}", browser.display()))
    };

    // Prefer the browser's sandbox; fall back only if it can't start (some
    // containers and locked-down systems), so the PDF still gets made.
    let outcome = run(true).or_else(|first| {
        run(false).map_err(|second| {
            if first == second {
                first
            } else {
                format!("{first} (retrying without the sandbox also failed: {second})")
            }
        })
    });
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

/// How long to wait for the browser before giving up.
const BROWSER_TIMEOUT: Duration = Duration::from_secs(120);

/// Run the browser until it has written `pdf`. Output goes to a log file rather than a
/// pipe: some browsers leave helper processes (crash reporter, GPU) alive after the main
/// process is done, and those hold a pipe open so that waiting for EOF would hang forever.
fn run_browser(cmd: &mut Command, pdf: &Path, log: &Path, timeout: Duration) -> Result<(), String> {
    let log_file =
        std::fs::File::create(log).map_err(|e| format!("could not create a log file: {e}"))?;
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone().map_err(|e| e.to_string())?)
        .stderr(log_file)
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;

    let started = Instant::now();
    let mut last_size = 0u64;
    let mut stable_polls = 0;
    let tail = || {
        std::fs::read_to_string(log)
            .ok()
            .and_then(|t| {
                t.lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "no output".to_string())
    };

    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() && pdf.is_file() {
                Ok(())
            } else {
                Err(format!(
                    "failed to print the PDF (exit {}): {}",
                    status.code().map_or("?".to_string(), |c| c.to_string()),
                    tail()
                ))
            };
        }

        // The PDF is complete once it stops growing; don't wait on lingering helpers.
        let size = std::fs::metadata(pdf).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size == last_size {
            stable_polls += 1;
            if stable_polls >= 10 {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
        } else {
            stable_polls = 0;
            last_size = size;
        }

        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "did not finish within {} seconds: {}",
                timeout.as_secs(),
                tail()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typography_and_elements() {
        let md = "# Introduction to DBMS\n\nThis is a paragraph with **bold text**, *italic text*, and `code snippet`.\n\n- Bullet point 1\n- Bullet point 2\n\n| Feature | DBMS | File System |\n| --- | --- | --- |\n| Redundancy | Minimal | High |\n| Concurrency | Supported | Poor |\n\n> Important Note: Always normalize tables.\n";
        let html = markdown_to_html(md, "DBMS Notes", None);

        for expected in [
            "<h1>Introduction to DBMS</h1>",
            "<strong>bold text</strong>",
            "<em>italic text</em>",
            "<code>code snippet</code>",
            "<ul>",
            "<li>Bullet point 1</li>",
            "<table>",
            "<th>Feature</th>",
            "<td>Minimal</td>",
            "<blockquote>Important Note: Always normalize tables.</blockquote>",
            "font-family: 'Inter'",
            "font-family: 'JetBrains Mono'",
        ] {
            assert!(html.contains(expected), "missing {expected}\n{html}");
        }
    }

    #[test]
    fn relative_images_are_inlined_as_base64() {
        let dir = std::env::temp_dir().join(format!("gulms-render-img-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("figures")).unwrap();
        let png = b"\x89PNG\r\n\x1a\nfake";
        std::fs::write(dir.join("figures/test_fig.png"), png).unwrap();

        let html = markdown_to_html("> ![Sample Figure](figures/test_fig.png)", "t", Some(&dir));

        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains(&base64::engine::general_purpose::STANDARD.encode(png)));
        assert!(html.contains(r#"<figure class="doc-figure">"#));
        assert!(html.contains("<figcaption>Sample Figure</figcaption>"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_is_escaped_so_code_in_lectures_survives() {
        let html = markdown_to_html(
            "- `#include <stdio.h>` and a < b && c > d\n\nTom & Jerry <b>",
            "A <title>",
            None,
        );

        assert!(
            html.contains("<code>#include &lt;stdio.h&gt;</code>"),
            "{html}"
        );
        assert!(html.contains("a &lt; b &amp;&amp; c &gt; d"));
        assert!(html.contains("<p>Tom &amp; Jerry &lt;b&gt;</p>"));
        assert!(html.contains("<title>A &lt;title&gt;</title>"));
    }

    #[test]
    fn bullets_nest_by_indentation() {
        let html = markdown_to_html("- a\n  - b\n    - c\n- d\n", "t", None);
        let flat: String = html
            .lines()
            .filter(|l| l.contains("<li>") || l.contains("ul>"))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            flat.contains(
                "<ul>  <li>a</li><ul>  <li>b</li><ul>  <li>c</li></ul></ul>  <li>d</li></ul>"
            ),
            "{flat}"
        );
    }

    #[test]
    fn a_figure_caption_travels_with_its_picture() {
        let html = markdown_to_html(
            "> **[Figure 1]**: Technical Figure (9x9 px)\n> ![cap](https://x/y.png)\n",
            "t",
            None,
        );
        let card = html
            .find(r#"<div class="figure-card"><blockquote>"#)
            .expect("caption and picture grouped");
        assert!(html[card..].contains(r#"<figure class="doc-figure">"#));
        assert!(html[card..].contains("</figure></div>"));
    }

    #[test]
    fn table_rule_rows_are_not_shown_as_data() {
        let html = markdown_to_html("| A | B |\n| --- | :---: |\n| 1 | 2 |\n\ntext", "t", None);
        assert!(!html.contains("---"), "{html}");
        assert_eq!(
            html.matches("<tr>").count(),
            2,
            "header + one data row: {html}"
        );
    }

    #[test]
    fn a_table_at_the_end_of_the_document_is_kept() {
        let html = markdown_to_html("# T\n\n| A | B |\n| --- | --- |\n| 1 | 2 |", "t", None);
        assert!(
            html.contains("<th>A</th>") && html.contains("<td>2</td>"),
            "{html}"
        );
    }

    #[test]
    fn mermaid_is_loaded_only_when_a_diagram_exists() {
        let without = markdown_to_html("# T\n\ntext", "t", None);
        assert!(
            !without.contains("<script"),
            "no script needed without diagrams"
        );

        let with = markdown_to_html(
            "```mermaid\nflowchart TD\n    a[\"x < y\"] --> b\n```\n",
            "t",
            None,
        );
        assert!(with.contains(MERMAID_SRI) && with.contains("mermaid@10.9.8"));
        assert!(with.contains("a[\"x &lt; y\"] --&gt; b"), "{with}");
    }

    #[test]
    fn code_fences_are_escaped_and_tagged() {
        let html = markdown_to_html("```c\nif (a < b) {}\n```\n", "t", None);
        assert!(
            html.contains(r#"<pre><code class="language-c">if (a &lt; b) {}</code></pre>"#),
            "{html}"
        );
    }

    #[test]
    fn file_urls_encode_spaces() {
        let url = file_url(Path::new("/tmp/a b/notes.html"));
        assert!(
            url.starts_with("file://") && url.contains("a%20b/notes.html"),
            "{url}"
        );
    }

    #[test]
    fn pdf_end_to_end_when_a_browser_is_installed() {
        if find_browser().is_none() {
            eprintln!("skipped: no Chromium-family browser installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("gulms-render-pdf-{}", std::process::id()));
        let out = dir.join("notes.pdf");
        let md = "# Lecture 05: Indexing\n\n## Slide 1: B+ Trees\n\nContent for slide 1.\n\n## Slide 2: Hash Indexes\n\nContent for slide 2.\n";

        render_markdown_to_pdf(md, &out, "Lecture 05: Indexing", None).expect("render");

        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"%PDF-") && bytes.len() > 1000);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("/Outlines"),
            "PDF should carry a navigable outline"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    mod browser_process {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A stand-in "browser": a shell script run with the arguments the real one gets.
        fn fake_browser(body: &str) -> (PathBuf, PathBuf) {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gulms-fakebrowser-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let script = dir.join("browser.sh");
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            (script, dir)
        }

        fn run(body: &str, timeout: Duration) -> (Result<(), String>, Duration) {
            let (script, dir) = fake_browser(body);
            let pdf = dir.join("out.pdf");
            let started = Instant::now();
            let result = run_browser(&mut Command::new(&script), &pdf, &dir.join("log"), timeout);
            let elapsed = started.elapsed();
            let _ = std::fs::remove_dir_all(&dir);
            (result, elapsed)
        }

        #[test]
        fn a_lingering_helper_process_does_not_hang_us() {
            // Writes the PDF and exits, but leaves a background child holding stdout/stderr
            // (like Chrome's crash reporter). A pipe-based wait would block until it dies.
            let (result, elapsed) = run(
                r#"printf '%%PDF-1.4 fake' > "$(dirname "$0")/out.pdf"; (sleep 20 &); exit 0"#,
                Duration::from_secs(10),
            );
            assert!(result.is_ok(), "{result:?}");
            assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
        }

        #[test]
        fn a_browser_that_finishes_the_pdf_but_never_exits_is_stopped() {
            let (result, elapsed) = run(
                r#"printf '%%PDF-1.4 fake' > "$(dirname "$0")/out.pdf"; sleep 20"#,
                Duration::from_secs(10),
            );
            assert!(result.is_ok(), "{result:?}");
            assert!(elapsed < Duration::from_secs(6), "took {elapsed:?}");
        }

        #[test]
        fn a_browser_that_hangs_without_output_times_out() {
            let (result, elapsed) = run("sleep 20", Duration::from_millis(700));
            let err = result.unwrap_err();
            assert!(err.contains("did not finish"), "{err}");
            assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
        }

        #[test]
        fn a_failing_browser_reports_its_last_output_line() {
            let (result, _) = run(
                "echo 'sandbox unavailable' >&2; exit 3",
                Duration::from_secs(10),
            );
            let err = result.unwrap_err();
            assert!(
                err.contains("exit 3") && err.contains("sandbox unavailable"),
                "{err}"
            );
        }
    }
}
