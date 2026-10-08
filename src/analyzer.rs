//! Reads a lecture number, title and agenda out of a `.pptx` so the many
//! near-duplicate uploads of a lecture can be recognised and given one name.
//!
//! This is a port of the original Python analyzer and must keep producing the
//! same `ppt_meta.json` entries (same fields, same `sha256` content hash) so
//! caches written by either version stay valid.

use regex::{Regex, RegexBuilder};
use roxmltree::Document;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::sync::LazyLock;

use crate::models::PptMeta;

const BOILERPLATE_PHRASES: &[&str] = &[
    "galgotias university",
    "school of computer science",
    "program name:",
    "b.tech",
    "course code:",
    "course name:",
    "prepared by",
    "vision & mission",
    "vision statement:",
    "mission statement:",
    "program educational objectives",
    "peo",
    "program outcomes",
    "pos",
    "course outcomes",
    "cos",
    "program specific outcomes",
    "psos",
    "‹#›",
    "m1:",
    "m2:",
    "m3:",
    "m4:",
    "reflect on the responses",
    "how these machines work",
    "at the end of this session",
    "learning outcomes:",
    "learning outcome",
];

fn ci(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("static regex")
}

static RE_SLIDE_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ppt/slides/slide\d+\.xml").unwrap());
static RE_FIRST_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
static RE_OUTLINE_HEAD: LazyLock<Regex> =
    LazyLock::new(|| ci(r"^(?:session|agenda|outline|objectives?)\b"));
static RE_LECTURE_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(?:session|lecture|lec|lesson)\s*(?:no\.?)?\s*[:\-\[]?\s*(\d+)"));
static RE_LECTURE_IN_NAME: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(?:session|lecture|lec|lesson|l)\s*[-_]?\s*(\d+)"));
static RE_LECTURE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| ci(r"^(?:session|lecture|lec|lesson)\s*(?:no\.?)?\s*[:\-\[]?\s*\d+"));
static RE_DANGLING_WORD: LazyLock<Regex> =
    LazyLock::new(|| ci(r"\b(?:vs|vs\.|and|or|of|to|in|for|with|the|&)\s*$"));
static RE_DANGLING_WORD_TRIM: LazyLock<Regex> =
    LazyLock::new(|| ci(r"\b(?:vs|vs\.|and|or|of|to|for|with|in|the|&)\s*$"));
static RE_NAME_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| ci(r"^(?:lesson|lecture|lec|session)[-_ ]*\d+[-_ ]*"));
static RE_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(.*?\)").unwrap());
static RE_UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[\\/*?:"<>|]"#).unwrap());
static RE_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_TRAILING_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s,;:\-]+$").unwrap());

/// File types whose content is an OOXML zip we can read. Legacy binary
/// `.ppt`/`.pps` files can only be named from their filename.
pub fn is_readable_deck(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".pptx") || lower.ends_with(".ppsx")
}

/// Analyse the bytes of a presentation. Never fails: anything that isn't a
/// readable `.pptx` falls back to information derived from the filename.
pub fn analyze_bytes(bytes: &[u8], raw_filename: &str) -> PptMeta {
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return fallback_info(raw_filename);
    };

    let mut slide_names: Vec<String> = zip
        .file_names()
        .filter(|n| RE_SLIDE_PART.is_match(n))
        .map(str::to_string)
        .collect();
    // Numeric order of the first number in the path (slide2 before slide10).
    slide_names.sort_by_key(|n| {
        RE_FIRST_NUMBER
            .find(n)
            .and_then(|m| m.as_str().parse::<u128>().ok())
            .unwrap_or(0)
    });

    let mut slides_clean: Vec<Vec<String>> = Vec::new();
    let mut outline_topics: Vec<String> = Vec::new();

    for name in &slide_names {
        let mut xml = String::new();
        let read = zip
            .by_name(name)
            .ok()
            .and_then(|mut f| f.read_to_string(&mut xml).ok());
        if read.is_none() {
            continue;
        }
        let Ok(doc) = Document::parse(&xml) else {
            continue;
        };

        let texts = element_texts(&doc);
        let clean: Vec<&String> = texts
            .iter()
            .filter(|t| {
                let low = t.to_lowercase();
                !BOILERPLATE_PHRASES.iter().any(|bp| low.contains(bp))
            })
            .collect();

        let is_outline_slide = texts.iter().any(|t| {
            let low = t.to_lowercase();
            low.contains("outline") || low.contains("agenda") || low.contains("objective")
        });
        if is_outline_slide {
            for t in &clean {
                if t.chars().count() > 3
                    && !RE_OUTLINE_HEAD.is_match(t)
                    && !outline_topics.contains(*t)
                {
                    outline_topics.push((*t).clone());
                }
            }
        }

        if !clean.is_empty() {
            slides_clean.push(clean.into_iter().cloned().collect());
        }
    }

    // 1. Lecture number: the intro slides first, then the filename.
    let full_intro = slides_clean
        .iter()
        .take(5)
        .map(|s| s.join(" "))
        .collect::<Vec<_>>()
        .join(" ");
    let lecture_num = RE_LECTURE_IN_TEXT
        .captures(&full_intro)
        .or_else(|| RE_LECTURE_IN_NAME.captures(raw_filename))
        .and_then(|c| c[1].parse::<u32>().ok());

    // 2. Main topic title, from the first slide that has real content.
    let title_candidates: Vec<&String> = slides_clean
        .first()
        .map(|first| {
            first
                .iter()
                .filter(|t| {
                    let low = t.to_lowercase();
                    const SKIP: &[&str] = &[
                        "course name",
                        "program name",
                        "prepared by",
                        "b.tech",
                        "galgotias",
                        "instructor name",
                        "students-centred",
                    ];
                    !SKIP.iter().any(|x| low.contains(x))
                        && !RE_LECTURE_PREFIX.is_match(t)
                        && t.chars().count() >= 2
                })
                .collect()
        })
        .unwrap_or_default();

    let mut main_title = if let Some(first) = title_candidates.first() {
        let mut title = (*first).clone();
        let mut idx = 1;
        // Join a title that was split over several text boxes ("Database System Vs" + "File System").
        while idx < title_candidates.len()
            && (RE_DANGLING_WORD.is_match(&title) || title.chars().count() < 10)
        {
            let next_part = title_candidates[idx];
            let low = next_part.to_lowercase();
            if [
                "instructor",
                "session",
                "course",
                "unit",
                "module",
                "galgotias",
            ]
            .iter()
            .any(|x| low.contains(x))
            {
                break;
            }
            let clean_next = next_part
                .split([',', ';'])
                .next()
                .unwrap_or_default()
                .trim();
            if !clean_next.is_empty() {
                title = format!("{} {}", title, clean_next).trim().to_string();
            }
            idx += 1;
        }
        title
    } else if let Some(first) = outline_topics.first() {
        first.clone()
    } else {
        let stem = file_stem(raw_filename);
        let stripped = RE_NAME_PREFIX.replace(stem, "");
        RE_PARENS.replace_all(&stripped, "").trim().to_string()
    };

    // Make it safe to use as part of a filename.
    main_title = RE_UNSAFE.replace_all(&main_title, " ").trim().to_string();
    main_title = RE_SPACES.replace_all(&main_title, " ").into_owned();
    main_title = RE_TRAILING_PUNCT.replace(&main_title, "").into_owned();
    main_title = RE_DANGLING_WORD_TRIM
        .replace(&main_title, "")
        .trim()
        .to_string();
    if main_title.is_empty() {
        main_title = "Lecture Material".to_string();
    }

    // 3. Canonical filename
    let short_title: String = main_title.chars().take(50).collect();
    let canonical_filename = format!(
        "{} - {}.pptx",
        lecture_prefix(lecture_num),
        short_title.trim()
    );

    // 4. Content hash: identical slide text means identical content.
    let normalized = slides_clean
        .iter()
        .map(|s| s.join(" "))
        .collect::<Vec<_>>()
        .join(" ");

    PptMeta {
        lecture_num,
        title: Some(main_title),
        canonical_filename: Some(canonical_filename),
        topics: outline_topics.into_iter().take(8).collect(),
        slide_count: Some(slide_names.len()),
        content_hash: Some(short_sha256(&normalized)),
        raw_filename: Some(raw_filename.to_string()),
    }
}

/// Best-effort information for files we can't open (legacy `.ppt`, corrupt zips).
pub fn fallback_info(raw_filename: &str) -> PptMeta {
    let stem = file_stem(raw_filename);
    let lecture_num = RE_LECTURE_IN_NAME
        .captures(stem)
        .and_then(|c| c[1].parse::<u32>().ok());
    let clean_title = RE_NAME_PREFIX.replace(stem, "").trim().to_string();

    PptMeta {
        lecture_num,
        title: Some(if clean_title.is_empty() {
            "Lecture".to_string()
        } else {
            clean_title.clone()
        }),
        canonical_filename: Some(format!(
            "{} - {}.pptx",
            lecture_prefix(lecture_num),
            clean_title
        )),
        topics: Vec::new(),
        slide_count: Some(0),
        content_hash: Some(short_sha256(raw_filename)),
        raw_filename: Some(raw_filename.to_string()),
    }
}

fn lecture_prefix(num: Option<u32>) -> String {
    match num {
        Some(n) => format!("Lec-{:02}", n),
        None => "Lec-??".to_string(),
    }
}

fn short_sha256(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()[..12]
        .to_string()
}

/// Filename without its last extension (`a.b.pptx` -> `a.b`).
fn file_stem(filename: &str) -> &str {
    match filename.rfind('.') {
        Some(i) if i > 0 => &filename[..i],
        _ => filename,
    }
}

/// The trimmed, non-empty leading text of every element, in document order.
fn element_texts(doc: &Document) -> Vec<String> {
    doc.descendants()
        .filter(|n| n.is_element())
        .filter_map(|n| {
            let first = n.first_child()?;
            if !first.is_text() {
                return None;
            }
            let trimmed = first.text()?.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect()
}

#[cfg(test)]
pub mod testdeck {
    //! Builds tiny but valid `.pptx` archives for tests.
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    pub const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

    /// One `<p:sp>` text box containing the given paragraphs.
    pub fn textbox(id: u32, paragraphs: &[&str]) -> String {
        let paras: String = paragraphs
            .iter()
            .map(|t| format!("<a:p><a:r><a:t>{}</a:t></a:r></a:p>", t))
            .collect();
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="TextBox {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="4000000" cy="600000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr><p:txBody><a:bodyPr/><a:p><a:endParaRPr/></a:p>{paras}</p:txBody></p:sp>"#,
            x = 500_000 + id * 10_000,
            y = 400_000 + id * 700_000,
        )
    }

    pub fn slide_xml(shapes: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{shapes}</p:spTree></p:cSld></p:sld>"#
        )
    }

    /// An archive with the given `(path, bytes)` entries.
    pub fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            for (name, data) in entries {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    /// A deck whose slides contain only the given text boxes (one list of paragraphs each).
    pub fn simple_deck(slides: &[Vec<&str>]) -> Vec<u8> {
        let entries: Vec<(String, Vec<u8>)> = slides
            .iter()
            .enumerate()
            .map(|(i, paras)| {
                let shapes: String = paras
                    .iter()
                    .enumerate()
                    .map(|(j, p)| textbox(2 + j as u32, &[p]))
                    .collect();
                (
                    format!("ppt/slides/slide{}.xml", i + 1),
                    slide_xml(&shapes).into_bytes(),
                )
            })
            .collect();
        let refs: Vec<(&str, Vec<u8>)> = entries
            .iter()
            .map(|(n, d)| (n.as_str(), d.clone()))
            .collect();
        zip_of(&refs)
    }
}

#[cfg(test)]
mod tests {
    use super::testdeck::*;
    use super::*;

    #[test]
    fn reads_lecture_number_title_and_slide_count() {
        let deck = simple_deck(&[
            vec![
                "Galgotias University",
                "Session 4",
                "Database System Vs",
                "File System",
            ],
            vec!["Outline", "Keys and attributes", "Normal forms"],
            vec!["Summary of the lecture"],
        ]);
        let meta = analyze_bytes(&deck, "DBMS_L4.pptx");

        assert_eq!(meta.lecture_num, Some(4));
        assert_eq!(
            meta.title.as_deref(),
            Some("Database System Vs File System")
        );
        assert_eq!(
            meta.canonical_filename.as_deref(),
            Some("Lec-04 - Database System Vs File System.pptx")
        );
        assert_eq!(meta.slide_count, Some(3));
        assert_eq!(meta.topics, vec!["Keys and attributes", "Normal forms"]);
        assert_eq!(meta.raw_filename.as_deref(), Some("DBMS_L4.pptx"));
    }

    #[test]
    fn identical_text_gives_identical_hash_and_changes_with_content() {
        let a = analyze_bytes(&simple_deck(&[vec!["Session 1", "Intro to SQL"]]), "a.pptx");
        let b = analyze_bytes(&simple_deck(&[vec!["Session 1", "Intro to SQL"]]), "b.pptx");
        let c = analyze_bytes(
            &simple_deck(&[vec!["Session 1", "Intro to NoSQL"]]),
            "a.pptx",
        );

        assert_eq!(a.content_hash, b.content_hash);
        assert_ne!(a.content_hash, c.content_hash);
        assert_eq!(a.content_hash.as_ref().unwrap().len(), 12);
    }

    #[test]
    fn slides_are_read_in_numeric_not_lexical_order() {
        let mut deck_slides = Vec::new();
        for i in 1..=11 {
            deck_slides.push(vec![if i == 5 {
                "Session 7"
            } else {
                "filler content"
            }]);
        }
        // Slide 5 is within the first five slides numerically, but lexically it sorts
        // after slide10 and slide11, which would push it out of the window.
        let meta = analyze_bytes(&simple_deck(&deck_slides), "x.pptx");
        assert_eq!(meta.lecture_num, Some(7));
        assert_eq!(meta.slide_count, Some(11));
    }

    #[test]
    fn falls_back_to_the_filename_when_slides_have_no_number() {
        let meta = analyze_bytes(
            &simple_deck(&[vec!["Arrays and Pointers"]]),
            "Lecture_12_Arrays.pptx",
        );
        assert_eq!(meta.lecture_num, Some(12));
        assert_eq!(meta.title.as_deref(), Some("Arrays and Pointers"));
    }

    #[test]
    fn non_zip_input_uses_filename_only() {
        let meta = analyze_bytes(b"\xd0\xcf\x11\xe0 legacy binary", "Lesson-3 Trees (v2).ppt");
        assert_eq!(meta.lecture_num, Some(3));
        assert_eq!(meta.slide_count, Some(0));
        assert_eq!(meta.title.as_deref(), Some("Trees (v2)"));
        assert_eq!(
            meta.canonical_filename.as_deref(),
            Some("Lec-03 - Trees (v2).pptx")
        );
    }

    #[test]
    fn titles_are_safe_for_filenames() {
        let meta = analyze_bytes(&simple_deck(&[vec!["Q&amp;A: What/Why? and"]]), "x.pptx");
        let title = meta.title.unwrap();
        assert!(!title.contains(['/', ':', '?']), "{title}");
        assert!(!title.ends_with("and"), "{title}");
    }

    #[test]
    fn readable_decks_are_ooxml_only() {
        assert!(is_readable_deck("a.PPTX"));
        assert!(is_readable_deck("a.ppsx"));
        assert!(!is_readable_deck("a.ppt"));
        assert!(!is_readable_deck("a.pps"));
    }

    /// Compares against the original Python analyzer on real decks.
    /// Run with: GULMS_PARITY_DECKS=/dir GULMS_PARITY_MANIFEST=manifest.json
    /// GULMS_PARITY_REF=ref_meta.json cargo test parity -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parity_with_python_analyzer() {
        let (Ok(decks), Ok(manifest), Ok(reference)) = (
            std::env::var("GULMS_PARITY_DECKS"),
            std::env::var("GULMS_PARITY_MANIFEST"),
            std::env::var("GULMS_PARITY_REF"),
        ) else {
            panic!("set GULMS_PARITY_DECKS, GULMS_PARITY_MANIFEST and GULMS_PARITY_REF");
        };
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
        let reference: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(reference).unwrap()).unwrap();

        let (mut checked, mut mismatches) = (0, Vec::new());
        for (key, expected) in reference.as_object().unwrap() {
            let bytes = std::fs::read(format!("{decks}/{key}.pptx")).unwrap();
            let name = manifest[key]["filename"].as_str().unwrap();
            let got = serde_json::to_value(analyze_bytes(&bytes, name)).unwrap();
            checked += 1;
            if &got != expected {
                mismatches.push(format!("{name}\n  want {expected}\n  got  {got}"));
            }
        }
        println!("checked {checked} decks, {} mismatches", mismatches.len());
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
