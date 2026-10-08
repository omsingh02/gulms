mod analyzer;
mod auth;
mod categorizer;
mod cli;
mod doctor;
mod downloader;
mod fsutil;
mod http;
mod interactive;
mod lectures;
mod models;
mod platform;
mod pptx;
mod render;
mod slide_extractor;
mod store;
mod sync;
mod ui;

use clap::{CommandFactory, Parser};
use colored::*;
use inquire::{InquireError, Select};
use std::io::IsTerminal;
use std::path::Path;

use cli::{Cli, Commands};
use downloader::download_material;
use lectures::{ExportOptions, Mode, ensure_analysed, export_course};
use models::Course;
use platform::open_file;
use store::{LmsStore, MaterialResult};
use ui::{BIN, badge, badge_width, truncate};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(e) = run() {
        // Esc / Ctrl-C at a prompt is a normal way out, not a failure to report.
        if let Some(InquireError::OperationCanceled | InquireError::OperationInterrupted) =
            e.downcast_ref::<InquireError>()
        {
            eprintln!("Cancelled.");
            std::process::exit(130);
        }
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

fn is_interactive_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Resolve a user-supplied course query (acronym, code, name, or index).
fn require_course<'a>(store: &'a LmsStore, query: &str) -> Result<&'a Course> {
    store.get_course(query).ok_or_else(|| {
        format!(
            "course '{}' not found (run `{} courses --all` to see what's available)",
            query, BIN
        )
        .into()
    })
}

/// Sync first when asked to (`--refresh`); a failed sync stops the command.
fn refresh_if(store: &mut LmsStore, refresh: bool) {
    if refresh && !sync::sync_and_report(store, false) {
        std::process::exit(1);
    }
}

/// Matches for a download/open query. Exact filename matches come first.
fn ranked_matches<'a>(
    store: &'a LmsStore,
    query: &str,
    course: Option<&str>,
) -> Result<Vec<MaterialResult<'a>>> {
    let mut results = store.search_materials(query, course, true);
    results.sort_by_key(|r| !r.material.filename.eq_ignore_ascii_case(query.trim()));
    if results.is_empty() {
        return Err(format!("no materials found matching '{}'", query).into());
    }
    Ok(results)
}

/// Pick the best match for a download/open query, telling the user when the
/// query was ambiguous.
fn pick_material<'a>(
    store: &'a LmsStore,
    query: &str,
    course: Option<&str>,
) -> Result<MaterialResult<'a>> {
    let mut results = ranked_matches(store, query, course)?;
    let total = results.len();
    let first = results.swap_remove(0);

    println!(
        "{} Found: [{}] {}",
        "•".cyan(),
        first.course.acronym,
        first.material.filename.bold()
    );
    if total > 1 {
        println!(
            "  {}",
            format!(
                "{} other {}; refine the query or narrow it with --course (see `{} search`)",
                total - 1,
                if total == 2 { "match" } else { "matches" },
                BIN
            )
            .dimmed()
        );
    }
    Ok(first)
}

/// Download every result; `dest` puts them all in one folder, otherwise each goes
/// to `<downloads>/<course>/<category>`.
fn download_all(store: &LmsStore, results: &[MaterialResult], dest: Option<&Path>) -> Result<()> {
    println!(
        "{} Downloading {} files...",
        "⬇".cyan().bold(),
        results.len()
    );
    let mut failed = 0;
    for r in results {
        if let Err(e) = download_material(r.material, &store.config, dest, true, None) {
            eprintln!("{} {}: {}", "✗".red(), r.material.filename, e);
            failed += 1;
        }
    }
    if failed > 0 {
        return Err(format!("{failed} download(s) failed").into());
    }
    Ok(())
}

fn print_results(results: &[MaterialResult], highlight_date: bool) {
    let width = badge_width(results.iter().map(|r| r.course.acronym.as_str()));
    for (idx, r) in results.iter().enumerate() {
        let date = r.material.date_modified_human();
        println!(
            "  {:>2}. {} {} {:<45} {:>9}  {}",
            idx + 1,
            badge(&r.course.acronym, width),
            r.material.category.icon(),
            r.material.filename,
            r.material.size_human().dimmed(),
            if highlight_date {
                date.yellow()
            } else {
                date.dimmed()
            }
        );
    }
    println!();
}

fn course_header(icon: &str, title: &str, c: &Course) {
    println!(
        "\n{} {} for [{}] {}:\n",
        icon.bold(),
        title,
        c.acronym.cyan().bold(),
        c.clean_name.bold()
    );
}

fn print_slides(store: &LmsStore, c: &Course, all: bool, outline: bool) {
    if all {
        let decks: Vec<_> = c.materials.iter().filter(|m| m.is_ppt()).collect();
        course_header("📑", &format!("All Slides ({} files)", decks.len()), c);
        println!("  {:<8} {:<52} SLIDES", "LEC", "FILE");
        for m in decks {
            let meta = store.get_ppt_meta(m.fileurl.as_deref().unwrap_or_default());
            let lec = match meta.and_then(|x| x.lecture_num) {
                Some(n) => format!("Lec-{n:02}"),
                None => "Lec-??".to_string(),
            };
            let name = meta
                .and_then(|x| x.canonical_filename.clone())
                .unwrap_or_else(|| m.filename.clone());
            let count = meta
                .and_then(|x| x.slide_count)
                .map_or("?".to_string(), |n| n.to_string());
            println!("  {:<8} {:<52} {}", lec.cyan(), name, count);
        }
        println!();
        return;
    }

    let (canonical, unnumbered) = c.canonical_slides(&store.ppt_meta);
    course_header(
        "📑",
        &format!(
            "Canonical Slides ({} canonical, {} extra)",
            canonical.len(),
            unnumbered.len()
        ),
        c,
    );

    for s in &canonical {
        println!(
            "  [Lec {:02}] {:<60} {:>10} {:>8}",
            s.lecture_num,
            truncate(&s.title, 60),
            format!("({} slides)", s.slide_count).yellow(),
            s.material.size_human().dimmed()
        );
        if outline
            && let Some(meta) =
                store.get_ppt_meta(s.material.fileurl.as_deref().unwrap_or_default())
            && !meta.topics.is_empty()
        {
            let preview = meta
                .topics
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(" • ");
            println!("           {}", format!("Outline: {preview}").dimmed());
        }
    }
    for m in &unnumbered {
        println!(
            "  [Extra ] {:<42} {:>10} {:>8}",
            m.filename,
            "",
            m.size_human().dimmed()
        );
    }
    println!();
}

fn run() -> Result<()> {
    let args = Cli::parse();

    // Needs no account, config or cache, so it must keep working even if those are broken.
    if let Some(Commands::Completions { shell }) = &args.command {
        clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
        return Ok(());
    }

    let mut store = LmsStore::load()?;

    let command = match args.command {
        Some(command) if !args.interactive => command,
        _ => {
            if !is_interactive_terminal() {
                // Nothing sensible to prompt; show usage instead of a cryptic TTY error.
                Cli::command().print_help()?;
                return Ok(());
            }
            // First run: walk the user through sign-in instead of showing an empty menu.
            if store.config.token.is_none() {
                auth::run_setup(&mut store)?;
            }
            return interactive::run_interactive(&mut store);
        }
    };

    match command {
        Commands::Setup => auth::run_setup(&mut store)?,
        Commands::Login {
            username,
            url,
            password_stdin,
        } => auth::login(
            &mut store,
            auth::LoginArgs {
                username,
                url,
                password_stdin,
            },
        )?,
        Commands::Logout => auth::logout(&mut store)?,
        Commands::Whoami => ui::print_account_info(&store),
        Commands::Doctor { offline } => {
            if !doctor::run(&store, offline) {
                std::process::exit(1);
            }
        }
        Commands::Completions { .. } => unreachable!("handled before the store is loaded"),
        Commands::Courses { all, refresh } => {
            refresh_if(&mut store, refresh);
            let courses = store.courses(all);
            if courses.is_empty() {
                println!(
                    "{}",
                    format!("No courses cached yet. Run `{} sync` first.", BIN).yellow()
                );
                return Ok(());
            }

            println!(
                "\n{} ({} total):\n",
                if all {
                    "All Enrolled Courses"
                } else {
                    "Tracked Courses"
                }
                .bold(),
                courses.len()
            );

            let width = badge_width(courses.iter().map(|c| c.acronym.as_str()));
            for (idx, c) in courses.iter().enumerate() {
                println!(
                    "  {:>2}. {} {:<45} {:>8} ({} items)",
                    idx + 1,
                    badge(&c.acronym, width),
                    c.clean_name,
                    c.total_size_human().dimmed(),
                    c.materials.len()
                );
            }
            println!();
        }
        Commands::Select => {
            if !is_interactive_terminal() {
                return Err("`select` needs an interactive terminal".into());
            }
            interactive::configure_courses_prompt(&mut store)?;
        }
        Commands::Sync { force } => {
            if !sync::sync_and_report(&mut store, force) {
                std::process::exit(1);
            }
        }
        Commands::Slides {
            course: Some(query),
            all,
            outline,
            refresh,
        } => {
            refresh_if(&mut store, refresh);
            let id = require_course(&store, &query)?.id;
            ensure_analysed(&mut store, &[id], Mode::Ask);
            print_slides(&store, require_course(&store, &query)?, all, outline);
        }
        Commands::Slides { course: None, .. } => {
            require_terminal("slides")?;
            interactive::browse_slides_menu(&mut store)?;
        }
        Commands::Analyze { course } => {
            let ids: Vec<u64> = match course {
                Some(q) => vec![require_course(&store, &q)?.id],
                None => store.courses(false).iter().map(|c| c.id).collect(),
            };
            if lectures::pending_jobs(&store, &ids).is_empty() {
                println!(
                    "{} Every slide deck is already analysed.",
                    "✓".green().bold()
                );
            } else if !ensure_analysed(&mut store, &ids, Mode::Auto) {
                return Err("some slide decks could not be analysed".into());
            }
        }
        Commands::Export {
            course,
            lecture,
            dest,
            no_pdf,
            no_ocr,
            open,
        } => {
            let id = require_course(&store, &course)?.id;
            export_course(
                &mut store,
                id,
                &ExportOptions {
                    lecture,
                    dest,
                    pdf: !no_pdf,
                    open,
                    ocr: !no_ocr,
                },
            )?;
        }
        Commands::Notes {
            course: Some(query),
        } => {
            let c = require_course(&store, &query)?;
            let by_cat = c.by_category();
            let notes = by_cat
                .get(&categorizer::Category::Notes)
                .cloned()
                .unwrap_or_default();
            course_header(
                "📝",
                &format!("Lecture Notes & Documents ({} items)", notes.len()),
                c,
            );

            for m in &notes {
                println!(
                    "  • {:<50} {:>9}  {}",
                    m.filename,
                    m.size_human().dimmed(),
                    m.date_modified_human().dimmed()
                );
            }
            println!();
        }
        Commands::Notes { course: None } => {
            require_terminal("notes")?;
            interactive::browse_notes_menu(&store)?;
        }
        Commands::View {
            course: Some(query),
            by_section,
            refresh,
        } => {
            refresh_if(&mut store, refresh);
            let c = require_course(&store, &query)?;
            course_header(
                "📦",
                &format!(
                    "Materials ({} items, {})",
                    c.materials.len(),
                    c.total_size_human()
                ),
                c,
            );

            if by_section {
                for section in &c.sections {
                    let files: Vec<_> = c
                        .materials
                        .iter()
                        .filter(|m| m.sections.contains(section))
                        .collect();
                    if files.is_empty() {
                        continue;
                    }
                    println!("  {} ({} files)", section.bold(), files.len());
                    for m in files {
                        println!("    • {:<48} {:>9}", m.filename, m.size_human().dimmed());
                    }
                    println!();
                }
            } else {
                for (cat, mats) in c.by_category() {
                    println!("  {} {} ({}):", cat.icon(), cat.name().bold(), mats.len());
                    for m in mats {
                        println!(
                            "    • {:<48} {:>9}  {}",
                            m.filename,
                            m.size_human().dimmed(),
                            m.date_modified_human().dimmed()
                        );
                    }
                }
                println!();
            }
        }
        Commands::View { course: None, .. } => {
            require_terminal("view")?;
            interactive::browse_courses(&mut store)?;
        }
        Commands::Search {
            query,
            course,
            download,
            dest,
        } => {
            let results = store.search_materials(&query, course.as_deref(), true);

            if results.is_empty() {
                println!(
                    "{}",
                    format!("No materials found matching '{}'.", query).yellow()
                );
                return Ok(());
            }

            println!(
                "\n{} Found {} materials matching '{}':\n",
                "🔍".bold(),
                results.len(),
                query.cyan()
            );
            print_results(&results, false);

            if download {
                download_all(&store, &results, dest.as_deref())?;
            } else if results.len() == 1 && is_interactive_terminal() {
                // A single hit in a terminal: go straight to the file actions.
                let first = results[0].material;
                let meta = store.get_ppt_meta(first.fileurl.as_deref().unwrap_or_default());
                interactive::handle_material_actions(first, &store.config, meta)?;
            }
        }
        Commands::Recent { days, all, dest } => {
            let results = store.recent_materials(days, all);
            if results.is_empty() {
                println!(
                    "{}",
                    format!(
                        "No materials uploaded or modified in the last {} days.",
                        days
                    )
                    .yellow()
                );
                return Ok(());
            }

            println!(
                "\n{} Materials modified in the last {} days ({} found):\n",
                "⚡".bold(),
                days,
                results.len()
            );
            print_results(&results, true);

            if let Some(dest) = dest {
                download_all(&store, &results, Some(&dest))?;
            }
        }
        Commands::Download {
            query,
            course,
            all,
            dest,
        } => {
            let results = ranked_matches(&store, &query, course.as_deref())?;
            if all {
                return download_all(&store, &results, dest.as_deref());
            }

            let chosen = if results.len() > 1 && is_interactive_terminal() {
                let labels: Vec<String> = results
                    .iter()
                    .map(|r| {
                        format!(
                            "[{}] {} ({})",
                            r.course.acronym,
                            r.material.filename,
                            r.material.size_human()
                        )
                    })
                    .collect();
                let picked = Select::new(
                    &format!("{} matches for '{}':", results.len(), query),
                    labels,
                )
                .with_page_size(15)
                .raw_prompt()?;
                results[picked.index].clone()
            } else {
                pick_material(&store, &query, course.as_deref())?
            };

            let path =
                download_material(chosen.material, &store.config, dest.as_deref(), true, None)
                    .map_err(|e| format!("download failed: {}", e))?;
            println!(
                "{} Download complete: {}",
                "✓".green().bold(),
                path.display()
            );
        }
        Commands::DownloadCourse {
            course,
            dest,
            yes,
            refresh,
        } => {
            refresh_if(&mut store, refresh);
            let id = require_course(&store, &course)?.id;
            lectures::download_course(&mut store, id, dest.as_deref(), yes)?;
        }
        Commands::Open { query, course } => {
            let found = pick_material(&store, &query, course.as_deref())?;
            let path = download_material(found.material, &store.config, None, true, None)
                .map_err(|e| format!("download failed: {}", e))?;
            println!("{} Opening file: {}", "✓".green(), path.display());
            open_file(&path, &store.config).map_err(|e| format!("failed to open file: {}", e))?;
        }
    }

    Ok(())
}

fn require_terminal(command: &str) -> Result<()> {
    if is_interactive_terminal() {
        Ok(())
    } else {
        Err(format!(
            "`{} {}` needs a course argument when not running in a terminal",
            BIN, command
        )
        .into())
    }
}
