use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static RE_HTML_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static RE_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(.*?\)").unwrap());
static RE_WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9]+").unwrap());
static RE_COURSE_CODE_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\([A-Z0-9_-]+\)\s*$").unwrap());
static RE_SAFE_FILENAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[\\/*?:"<>|]"#).unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Syllabus,
    Textbooks,
    Slides,
    Notes,
    Code,
    WebLinks,
    Other,
}

impl Category {
    pub fn name(&self) -> &'static str {
        match self {
            Category::Syllabus => "Syllabus & Session Plans",
            Category::Textbooks => "Textbooks & Course Packs",
            Category::Slides => "Lecture Slides (PPT)",
            Category::Notes => "Lecture Notes & Documents",
            Category::Code => "Code & Archives",
            Category::WebLinks => "Web Links & Videos",
            Category::Other => "Other Materials",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Category::Syllabus => "📋",
            Category::Textbooks => "📚",
            Category::Slides => "📑",
            Category::Notes => "📝",
            Category::Code => "📦",
            Category::WebLinks => "🔗",
            Category::Other => "📄",
        }
    }

    pub fn all() -> &'static [Category] {
        &[
            Category::Slides,
            Category::Textbooks,
            Category::Notes,
            Category::Syllabus,
            Category::Code,
            Category::WebLinks,
            Category::Other,
        ]
    }
}

pub fn clean_text(text: &str) -> String {
    let unescaped = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&nbsp;", " ");
    RE_HTML_TAGS.replace_all(&unescaped, "").trim().to_string()
}

pub fn clean_display_name(raw_name: &str) -> String {
    let cleaned = clean_text(raw_name);
    let stripped = RE_COURSE_CODE_SUFFIX.replace_all(&cleaned, "").trim().to_string();
    if stripped.is_empty() {
        cleaned
    } else {
        stripped
    }
}

pub fn get_acronym(course_name: &str) -> String {
    let cleaned = RE_PARENS.replace_all(&clean_text(course_name), "").to_string();
    let stop_words: HashSet<&'static str> = [
        "and", "with", "using", "of", "the", "for", "in", "to"
    ].into_iter().collect();

    let mut acronym = String::new();
    for mat in RE_WORDS.find_iter(&cleaned) {
        let w = mat.as_str();
        if !stop_words.contains(w.to_lowercase().as_str()) {
            if let Some(ch) = w.chars().next() {
                acronym.push(ch.to_ascii_uppercase());
            }
        }
    }
    acronym
}

pub fn categorize_file(fname: &str, fsize: u64, mtype: &str) -> Category {
    let cleaned = clean_text(fname);
    let lower = cleaned.to_lowercase();
    let ext = std::path::Path::new(&cleaned)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{}", s.to_lowercase()))
        .unwrap_or_default();

    if ext == ".xls" || ext == ".xlsx" || lower.contains("session plan") || lower.contains("syllabus") {
        return Category::Syllabus;
    }
    if lower.contains("book")
        || lower.contains("course pack")
        || lower.contains("coursepack")
        || lower.contains("textbook")
        || (ext == ".pdf" && fsize > 6 * 1024 * 1024)
    {
        return Category::Textbooks;
    }
    if ext == ".pptx" || ext == ".ppt" || ext == ".pps" || ext == ".ppsx" {
        return Category::Slides;
    }
    if [".zip", ".rar", ".tar", ".gz", ".7z", ".java", ".py", ".c", ".cpp", ".sql"]
        .contains(&ext.as_str())
    {
        return Category::Code;
    }
    if [".doc", ".docx", ".pdf", ".txt", ".odt"].contains(&ext.as_str()) {
        return Category::Notes;
    }
    if mtype == "url" {
        return Category::WebLinks;
    }

    Category::Other
}

pub fn format_size(num_bytes: u64) -> String {
    if num_bytes == 0 {
        return "0 B".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = num_bytes as f64;
    for (i, unit) in units.iter().enumerate() {
        if size < 1024.0 || i == units.len() - 1 {
            if *unit == "B" {
                return format!("{:.0} B", size);
            } else {
                return format!("{:.1} {}", size, unit);
            }
        }
        size /= 1024.0;
    }
    format!("{:.1} TB", size)
}

pub fn sanitize_filename(name: &str) -> String {
    let stripped = RE_HTML_TAGS.replace_all(name, "").to_string();
    RE_SAFE_FILENAME.replace_all(&stripped, "_").trim().to_string()
}
