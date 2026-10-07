mod categorizer;
mod cli;
mod downloader;
mod interactive;
mod models;
mod store;
mod sync;

use clap::Parser;
use colored::*;
use std::io::IsTerminal;

use cli::{Cli, Commands};
use downloader::{download_material, open_file};
use store::LmsStore;
use sync::LmsClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    let mut store = LmsStore::load()?;

    match args.command {
        None => {
            // Default to interactive mode
            interactive::run_interactive(&mut store)?;
        }
        Some(Commands::Whoami) => {
            println!("\n{}", "═══ GULMS Account & System Info ═══".bold().cyan());
            println!("  {:<18} {}", "Base Portal:".bold(), store.config.base_url);
            println!(
                "  {:<18} {}",
                "Student Name:".bold(),
                store.config.fullname.as_deref().unwrap_or("Not set")
            );
            println!(
                "  {:<18} {}",
                "Username:".bold(),
                store.config.username.as_deref().unwrap_or("Not set")
            );
            println!(
                "  {:<18} {}",
                "User ID:".bold(),
                store
                    .config
                    .user_id
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "Not set".to_string())
            );
            println!(
                "  {:<18} {}",
                "Token Configured:".bold(),
                if store.config.token.is_some() {
                    "Yes".green()
                } else {
                    "No (Run setup in ~/.config/gulms/config.json)".red()
                }
            );
            println!(
                "  {:<18} {}",
                "Download Directory:".bold(),
                store.download_dir().display().to_string().cyan()
            );
            println!(
                "  {:<18} {}",
                "Courses Cache:".bold(),
                store.courses_cache_path.display()
            );
            println!(
                "  {:<18} {} courses loaded",
                "Cached Courses:".bold(),
                store.courses.len()
            );

            if let Some(ref sync_state) = store.sync_state {
                println!(
                    "  {:<18} {}",
                    "Last Sync:".bold(),
                    sync_state.last_sync_human.as_deref().unwrap_or("Unknown").yellow()
                );
            }
            println!();
        }
        Some(Commands::Courses { all }) => {
            let courses = store.courses(all);
            println!(
                "\n{} ({} total):\n",
                if all { "All Enrolled Courses".bold() } else { "Tracked Courses".bold() },
                courses.len()
            );

            for (idx, c) in courses.iter().enumerate() {
                let badge = format!("[{}]", c.acronym).bold().cyan();
                println!(
                    "  {:>2}. {:<7} {:<45} {:>8} ({} items)",
                    idx + 1,
                    badge,
                    c.clean_name,
                    c.total_size_human().dimmed(),
                    c.materials.len()
                );
            }
            println!();
        }
        Some(Commands::Select) => {
            interactive::run_interactive(&mut store)?;
        }
        Some(Commands::Sync { force }) => {
            let mut client = LmsClient::new(
                store.config.base_url.clone(),
                store.config.token.clone(),
                store.config.user_id,
            );
            println!(
                "\n{} Running {} sync with Galgotias LMS...",
                "🔄".cyan().bold(),
                if force { "Full Force" } else { "Incremental Delta" }
            );

            match client.sync(&mut store, force) {
                Ok(stats) => {
                    println!(
                        "\n{} Sync successful in {:.2}s!",
                        "✓".green().bold(),
                        stats.elapsed.as_secs_f64()
                    );
                    println!(
                        "  Checked: {} courses | Modified/Fetched: {} | New files: {}",
                        stats.checked_count,
                        stats.updated_courses.len(),
                        stats.new_files_count
                    );
                }
                Err(e) => {
                    eprintln!("\n{} Sync failed: {}", "✗".red().bold(), e);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Slides { course }) => {
            if let Some(c_query) = course {
                if let Some(c) = store.get_course(&c_query) {
                    let (canonical, unnumbered) = c.canonical_slides(&store.ppt_meta);
                    println!(
                        "\n{} Canonical Slides for [{}] {} ({} canonical, {} extra):\n",
                        "📑".bold(),
                        c.acronym.cyan().bold(),
                        c.clean_name.bold(),
                        canonical.len(),
                        unnumbered.len()
                    );

                    for s in &canonical {
                        println!(
                            "  [Lec {:02}] {:<42} {:>10} {:>8}",
                            s.lecture_num,
                            s.title,
                            format!("({} slides)", s.slide_count).yellow(),
                            s.material.size_human().dimmed()
                        );
                    }
                    for m in &unnumbered {
                        println!(
                            "  [Extra ] {:<42} {:>8}",
                            m.filename,
                            m.size_human().dimmed()
                        );
                    }
                    println!();
                } else {
                    eprintln!("{} Course '{}' not found.", "✗".red(), c_query);
                }
            } else if std::io::stdout().is_terminal() {
                interactive::run_interactive(&mut store)?;
            }
        }
        Some(Commands::Notes { course }) => {
            if let Some(c_query) = course {
                if let Some(c) = store.get_course(&c_query) {
                    let by_cat = c.by_category();
                    let notes = by_cat.get(&categorizer::Category::Notes).cloned().unwrap_or_default();
                    println!(
                        "\n{} Lecture Notes & Documents for [{}] {} ({} items):\n",
                        "📝".bold(),
                        c.acronym.cyan().bold(),
                        c.clean_name.bold(),
                        notes.len()
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
                } else {
                    eprintln!("{} Course '{}' not found.", "✗".red(), c_query);
                }
            } else if std::io::stdout().is_terminal() {
                interactive::run_interactive(&mut store)?;
            }
        }
        Some(Commands::View { course }) => {
            if let Some(c_query) = course {
                if let Some(c) = store.get_course(&c_query) {
                    println!(
                        "\n{} Materials for [{}] {} ({} items, {}):\n",
                        "📦".bold(),
                        c.acronym.cyan().bold(),
                        c.clean_name.bold(),
                        c.materials.len(),
                        c.total_size_human().yellow()
                    );

                    let by_cat = c.by_category();
                    for (cat, mats) in by_cat {
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
                } else {
                    eprintln!("{} Course '{}' not found.", "✗".red(), c_query);
                }
            } else if std::io::stdout().is_terminal() {
                interactive::run_interactive(&mut store)?;
            }
        }
        Some(Commands::Search { query, course }) => {
            let results = store.search_materials(&query, course.as_deref(), true);

            if results.is_empty() {
                println!("{}", format!("No materials found matching '{}'.", query).yellow());
                return Ok(());
            }

            println!(
                "\n{} Found {} materials matching '{}':\n",
                "🔍".bold(),
                results.len(),
                query.cyan()
            );

            for (idx, r) in results.iter().enumerate() {
                println!(
                    "  {:>2}. [{}] {} {:<45} {:>9}  {}",
                    idx + 1,
                    r.course.acronym.cyan().bold(),
                    r.material.category.icon(),
                    r.material.filename,
                    r.material.size_human().dimmed(),
                    r.material.date_modified_human().dimmed()
                );
            }
            println!();

            // If interactive terminal and single result, offer to open
            if results.len() == 1 && std::io::stdout().is_terminal() {
                let first = results[0].material;
                let meta = store.get_ppt_meta(first.fileurl.as_deref().unwrap_or_default());
                interactive::handle_material_actions(first, &store.config, meta)?;
            }
        }
        Some(Commands::Recent { days, all }) => {
            let results = store.recent_materials(days, all);
            if results.is_empty() {
                println!(
                    "{}",
                    format!("No materials uploaded or modified in the last {} days.", days).yellow()
                );
                return Ok(());
            }

            println!(
                "\n{} Materials modified in the last {} days ({} found):\n",
                "⚡".bold(),
                days,
                results.len()
            );

            for (idx, r) in results.iter().enumerate() {
                println!(
                    "  {:>2}. [{}] {} {:<45} {:>9}  {}",
                    idx + 1,
                    r.course.acronym.cyan().bold(),
                    r.material.category.icon(),
                    r.material.filename,
                    r.material.size_human().dimmed(),
                    r.material.date_modified_human().yellow()
                );
            }
            println!();
        }
        Some(Commands::Download { query, course }) => {
            let results = store.search_materials(&query, course.as_deref(), true);

            if results.is_empty() {
                eprintln!("{} No materials found matching '{}'.", "✗".red(), query);
                return Ok(());
            }

            let target = results[0].material;
            println!(
                "{} Found: [{}] {}",
                "•".cyan(),
                results[0].course.acronym,
                target.filename.bold()
            );

            match download_material(target, &store.config, None, true) {
                Ok(path) => println!("{} Download complete: {}", "✓".green().bold(), path.display()),
                Err(e) => eprintln!("{} Download failed: {}", "✗".red(), e),
            }
        }
        Some(Commands::Open { query, course }) => {
            let results = store.search_materials(&query, course.as_deref(), true);

            if results.is_empty() {
                eprintln!("{} No materials found matching '{}'.", "✗".red(), query);
                return Ok(());
            }

            let target = results[0].material;
            println!(
                "{} Found: [{}] {}",
                "•".cyan(),
                results[0].course.acronym,
                target.filename.bold()
            );

            match download_material(target, &store.config, None, true) {
                Ok(path) => {
                    println!("{} Opening file: {}", "✓".green(), path.display());
                    if let Err(e) = open_file(&path) {
                        eprintln!("{} Failed to open file: {}", "⚠".yellow(), e);
                    }
                }
                Err(e) => eprintln!("{} Download failed: {}", "✗".red(), e),
            }
        }
    }

    Ok(())
}
