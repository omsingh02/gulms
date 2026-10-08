//! Lecture-level features built on the analyzer and extractor: finding out what
//! each slide deck is, exporting study notes, and downloading a course with
//! tidy lecture names.

use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use inquire::Confirm;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use crate::analyzer;
use crate::categorizer::{format_size, sanitize_filename};
use crate::downloader::{download_material, fetch_bytes};
use crate::fsutil::atomic_write;
use crate::models::{Config, Course, Material, PptMeta};
use crate::platform::open_file;
use crate::render::render_markdown_to_pdf;
use crate::slide_extractor::{ExtractOptions, extract_deck};
use crate::store::LmsStore;
use crate::ui::BIN;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Decks are fetched this many at a time: enough to be quick, few enough to be polite.
const WORKERS: usize = 4;

// ------------------------------------------------------------------- analysis

#[derive(Clone)]
pub struct Job {
    pub key: String,
    pub material: Material,
}

/// Slide decks in these courses that have not been analysed yet.
pub fn pending_jobs(store: &LmsStore, course_ids: &[u64]) -> Vec<Job> {
    let mut seen = HashSet::new();
    store
        .courses
        .iter()
        .filter(|c| course_ids.contains(&c.id))
        .flat_map(|c| c.materials.iter())
        .filter(|m| m.is_ppt() && m.fileurl.is_some())
        .filter_map(|m| {
            let key = m.ppt_cache_key()?;
            (!store.ppt_meta.contains_key(&key) && seen.insert(key.clone())).then(|| Job {
                key,
                material: m.clone(),
            })
        })
        .collect()
}

fn analyse_one(job: &Job, config: &Config) -> std::result::Result<PptMeta, String> {
    let name = &job.material.filename;
    // Legacy binary formats can't be read; their filename is all we have, no download needed.
    if !analyzer::is_readable_deck(name) {
        return Ok(analyzer::fallback_info(name));
    }

    let mut last_error = String::new();
    for _ in 0..2 {
        match fetch_bytes(&job.material, config) {
            Ok(bytes) if bytes.starts_with(b"PK") => {
                return Ok(analyzer::analyze_bytes(&bytes, name));
            }
            Ok(_) => {
                return Err(
                    "the portal did not return a presentation (try `gulms login` again)"
                        .to_string(),
                );
            }
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(last_error)
}

pub struct AnalysisReport {
    pub analysed: usize,
    pub failed: Vec<(String, String)>,
}

/// Download and analyse `jobs` in parallel, saving results as they arrive.
pub fn run_analysis(store: &mut LmsStore, jobs: Vec<Job>) -> AnalysisReport {
    let total = jobs.len();
    let bar = ProgressBar::new(total as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:30.cyan/blue}] {pos}/{len} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("━╸ "),
    );

    let config = store.config.clone();
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, std::result::Result<PptMeta, String>)>();
    let mut report = AnalysisReport {
        analysed: 0,
        failed: Vec::new(),
    };
    let mut since_save = 0;

    std::thread::scope(|scope| {
        let (jobs, config, next) = (&jobs, &config, &next);
        for _ in 0..WORKERS.min(total) {
            let tx = tx.clone();
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(job) = jobs.get(i) else { break };
                    if tx.send((i, analyse_one(job, config))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        for (i, result) in rx {
            let job = &jobs[i];
            bar.set_message(job.material.filename.clone());
            match result {
                Ok(meta) => {
                    store.ppt_meta.insert(job.key.clone(), meta);
                    report.analysed += 1;
                }
                Err(reason) => report.failed.push((job.material.filename.clone(), reason)),
            }
            bar.inc(1);
            // Keep progress if the user interrupts a long run.
            since_save += 1;
            if since_save >= 25 {
                since_save = 0;
                let _ = store.save_ppt_meta();
            }
        }
    });
    bar.finish_and_clear();

    if let Err(e) = store.save_ppt_meta() {
        eprintln!(
            "{} could not save lecture data: {}",
            "warning:".yellow().bold(),
            e
        );
    }
    report
}

pub enum Mode {
    /// Analyse without asking (the command needs it to do its job).
    Auto,
    /// Ask first in a terminal; elsewhere just print a hint.
    Ask,
}

/// Make sure the slide decks of these courses are analysed. Returns false if some
/// were left unanalysed (declined, no terminal, or download failures).
pub fn ensure_analysed(store: &mut LmsStore, course_ids: &[u64], mode: Mode) -> bool {
    let jobs = pending_jobs(store, course_ids);
    if jobs.is_empty() {
        return true;
    }
    let download_bytes: u64 = jobs
        .iter()
        .filter(|j| analyzer::is_readable_deck(&j.material.filename))
        .map(|j| j.material.filesize)
        .sum();

    match mode {
        Mode::Auto => {}
        Mode::Ask => {
            if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
                eprintln!(
                    "{} {} slide files aren't analysed yet, so lectures can't be numbered or titled. Run `{} analyze` to fix that.",
                    "note:".yellow().bold(),
                    jobs.len(),
                    BIN
                );
                return false;
            }
            let question = format!(
                "{} slide files haven't been analysed yet (downloads about {}). Analyse them now to number and title the lectures?",
                jobs.len(),
                format_size(download_bytes)
            );
            if !matches!(
                Confirm::new(&question).with_default(true).prompt(),
                Ok(true)
            ) {
                return false;
            }
        }
    }

    println!(
        "{} Analysing {} slide files ({})...",
        "•".cyan().bold(),
        jobs.len(),
        format_size(download_bytes)
    );
    let report = run_analysis(store, jobs);
    println!(
        "{} Analysed {} slide files",
        "✓".green().bold(),
        report.analysed
    );
    for (name, reason) in report.failed.iter().take(5) {
        eprintln!("  {} {}: {}", "⚠".yellow(), name, reason);
    }
    if report.failed.len() > 5 {
        eprintln!("  ... and {} more", report.failed.len() - 5);
    }
    report.failed.is_empty()
}

// --------------------------------------------------------------------- export

pub struct ExportOptions {
    pub lecture: Option<u32>,
    pub dest: Option<PathBuf>,
    pub pdf: bool,
    pub open: bool,
    pub ocr: bool,
}

/// A filesystem- and link-friendly name for a lecture's figures folder.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn file_stem(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
}

/// Where a course's study notes go: `<dest or downloads>/<course>/Study Notes`.
pub fn notes_dir(course: &Course, config: &Config, dest: Option<&Path>) -> PathBuf {
    let base = dest
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config.download_dir());
    let name = if course.acronym.is_empty() {
        &course.clean_name
    } else {
        &course.acronym
    };
    base.join(sanitize_filename(name)).join("Study Notes")
}

/// Export study notes for the lectures of one course.
pub fn export_course(
    store: &mut LmsStore,
    course_id: u64,
    opts: &ExportOptions,
) -> Result<Vec<PathBuf>> {
    ensure_analysed(store, &[course_id], Mode::Auto);

    let course = store
        .courses
        .iter()
        .find(|c| c.id == course_id)
        .ok_or("course not found")?;
    let (canonical, _) = course.canonical_slides(&store.ppt_meta);
    if canonical.is_empty() {
        return Err(format!("no numbered lecture slides found for {}", course.clean_name).into());
    }

    let targets: Vec<_> = match opts.lecture {
        Some(n) => {
            let hit: Vec<_> = canonical.iter().filter(|c| c.lecture_num == n).collect();
            if hit.is_empty() {
                let available: Vec<String> = canonical
                    .iter()
                    .map(|c| c.lecture_num.to_string())
                    .collect();
                return Err(format!(
                    "lecture {} not found in {} (available: {})",
                    n,
                    course.acronym,
                    available.join(", ")
                )
                .into());
            }
            hit
        }
        None => canonical.iter().collect(),
    };

    let out_dir = notes_dir(course, &store.config, opts.dest.as_deref());
    std::fs::create_dir_all(&out_dir)?;
    println!(
        "\n{} Exporting study notes for [{}] {} ({} lectures)...\n",
        "🧾".bold(),
        course.acronym.cyan().bold(),
        course.clean_name.bold(),
        targets.len()
    );

    let mut produced: Vec<PathBuf> = Vec::new();
    let mut pdf_problem: Option<String> = None;
    let mut failures = 0;

    for slide in targets {
        let material = slide.material;
        let Some(meta) = store.get_ppt_meta(material.fileurl.as_deref().unwrap_or_default()) else {
            continue;
        };
        let canonical_name = meta
            .canonical_filename
            .clone()
            .unwrap_or_else(|| material.filename.clone());
        let stem = sanitize_filename(file_stem(&canonical_name));
        let label = format!("Lec-{:02}", slide.lecture_num);
        print!(
            "  {} {:<46} ",
            label.cyan(),
            crate::ui::truncate(&slide.title, 46)
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());

        let bytes = match fetch_bytes(material, &store.config) {
            Ok(b) if b.starts_with(b"PK") => b,
            Ok(_) => {
                println!("{}", "✗ the portal did not return a presentation".red());
                failures += 1;
                continue;
            }
            Err(e) => {
                println!("{} {}", "✗".red(), e);
                failures += 1;
                continue;
            }
        };

        let extract = ExtractOptions {
            figures_dir: Some(out_dir.join("figures").join(slug(&stem))),
            figures_href: format!("figures/{}", slug(&stem)),
            ocr: opts.ocr,
            ..ExtractOptions::recommended()
        };
        let slides = match extract_deck(&bytes, &extract) {
            Ok(s) => s,
            Err(e) => {
                println!("{} {}", "✗".red(), e);
                failures += 1;
                continue;
            }
        };

        let lec_str = format!("Lecture {}: ", slide.lecture_num);
        let base_title = meta.title.clone().unwrap_or_else(|| stem.clone());
        let mut doc = vec![
            format!("# {lec_str}{base_title}\n"),
            format!(
                "> Source: `{}` ({} slides)\n",
                material.filename,
                slides.len()
            ),
            "---\n".to_string(),
        ];
        for s in &slides {
            let suffix = if s.title.is_empty() {
                String::new()
            } else {
                format!(": {}", s.title)
            };
            doc.push(format!("## Slide {}{}\n", s.slide_num, suffix));
            let heading = format!("# {}", s.title);
            let body = if !s.title.is_empty() && s.markdown.starts_with(&heading) {
                s.markdown[heading.len()..].trim_start().to_string()
            } else {
                s.markdown.clone()
            };
            doc.push(body);
            doc.push("\n---\n".to_string());
        }
        let markdown = doc.join("\n");

        let md_path = out_dir.join(format!("{stem}.md"));
        atomic_write(&md_path, markdown.as_bytes(), false)?;
        let mut shown = md_path.clone();

        if opts.pdf && pdf_problem.is_none() {
            let pdf_path = out_dir.join(format!("{stem}.pdf"));
            match render_markdown_to_pdf(
                &markdown,
                &pdf_path,
                &format!("{lec_str}{base_title}"),
                Some(&out_dir),
            ) {
                Ok(()) => shown = pdf_path,
                Err(e) => pdf_problem = Some(e),
            }
        }

        println!(
            "{} {}",
            "✓".green().bold(),
            shown
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        );
        produced.push(shown);
    }

    println!("\nNotes saved to: {}", out_dir.display().to_string().bold());
    if let Some(problem) = pdf_problem {
        eprintln!(
            "{} PDF output skipped: {}",
            "note:".yellow().bold(),
            problem
        );
    }
    if opts.open {
        match produced.as_slice() {
            [only] => {
                if let Err(e) = open_file(only, &store.config) {
                    eprintln!("{} {}", "warning:".yellow().bold(), e);
                }
            }
            _ => eprintln!(
                "{} --open only works when exporting a single lecture",
                "note:".yellow().bold()
            ),
        }
    }
    if failures > 0 {
        return Err(format!("{failures} lecture(s) could not be exported").into());
    }
    Ok(produced)
}

// ------------------------------------------------------------ download-course

/// Download a whole course: one copy of each lecture (the best version, named
/// `Lec-NN - Title.pptx`), plus every other file, grouped by category.
pub fn download_course(
    store: &mut LmsStore,
    course_id: u64,
    dest: Option<&Path>,
    assume_yes: bool,
) -> Result<()> {
    ensure_analysed(store, &[course_id], Mode::Auto);

    let course = store
        .courses
        .iter()
        .find(|c| c.id == course_id)
        .ok_or("course not found")?;
    let (canonical, unnumbered) = course.canonical_slides(&store.ppt_meta);

    let mut files: Vec<(&Material, String)> = Vec::new();
    for slide in &canonical {
        let name = store
            .get_ppt_meta(slide.material.fileurl.as_deref().unwrap_or_default())
            .and_then(|m| m.canonical_filename.clone())
            .unwrap_or_else(|| slide.material.filename.clone());
        files.push((slide.material, name));
    }
    for m in unnumbered {
        files.push((m, m.filename.clone()));
    }
    for m in course.materials.iter().filter(|m| !m.is_ppt()) {
        files.push((m, m.filename.clone()));
    }
    files.retain(|(m, _)| m.fileurl.is_some());

    if files.is_empty() {
        println!("No files found.");
        return Ok(());
    }

    let total: u64 = files.iter().map(|(m, _)| m.filesize).sum();
    let folder_name = if course.acronym.is_empty() {
        course.id.to_string()
    } else {
        course.acronym.clone()
    };
    let base = dest
        .map(Path::to_path_buf)
        .unwrap_or_else(|| store.config.download_dir())
        .join(sanitize_filename(&folder_name));
    println!(
        "\n{} {} ({} files, {}) to {}",
        "Downloading".bold(),
        folder_name.cyan().bold(),
        files.len(),
        format_size(total),
        base.display()
    );

    if !assume_yes {
        if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
            return Err(
                "not running in a terminal; pass --yes to download without a prompt".into(),
            );
        }
        if !matches!(
            Confirm::new("Proceed?").with_default(false).prompt(),
            Ok(true)
        ) {
            return Ok(());
        }
    }

    let (mut done, mut cached, mut failed) = (0, 0, 0);
    for (i, (material, name)) in files.iter().enumerate() {
        let dir = base.join(sanitize_filename(material.category.name()));
        let target = dir.join(sanitize_filename(name));
        let position = format!("[{:>2}/{}]", i + 1, files.len());

        if material.filesize > 0
            && std::fs::metadata(&target).is_ok_and(|m| m.len() == material.filesize)
        {
            println!("  {} {} {}", position, name, "(cached)".dimmed());
            cached += 1;
            continue;
        }
        print!("  {} {} ({})... ", position, name, material.size_human());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        match download_material(material, &store.config, Some(&dir), false, Some(name)) {
            Ok(_) => {
                println!("{}", "done".green());
                done += 1;
            }
            Err(e) => {
                println!("{} {}", "failed:".red(), e);
                failed += 1;
            }
        }
    }

    println!(
        "\n{} {} downloaded, {} already present, {} failed. Files are in {}",
        if failed == 0 {
            "✓".green().bold()
        } else {
            "!".yellow().bold()
        },
        done,
        cached,
        failed,
        base.display()
    );
    if failed > 0 {
        return Err(format!("{failed} file(s) failed to download").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_link_friendly() {
        assert_eq!(
            slug("Lec-04 - Database System Vs File System"),
            "lec-04-database-system-vs-file-system"
        );
        assert_eq!(slug("  Q&A: What?  "), "q-a-what");
        assert_eq!(slug("Lec-?? - x"), "lec-x");
    }

    #[test]
    fn stems_drop_only_the_extension() {
        assert_eq!(file_stem("Lec-01 - Intro.v2.pptx"), "Lec-01 - Intro.v2");
    }
}
