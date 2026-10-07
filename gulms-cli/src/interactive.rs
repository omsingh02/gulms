use colored::*;
use inquire::{Confirm, MultiSelect, Select, Text};
use std::fmt;
use std::path::PathBuf;
use std::process::Command;

use crate::categorizer::Category;
use crate::downloader::{attach_token, copy_to_clipboard, download_material, open_file};
use crate::models::{CanonicalSlide, Course, Material, PptMeta};
use crate::store::LmsStore;
use crate::sync::LmsClient;

// Wrapper for Course in Inquire menus
struct CourseItem<'a> {
    course: &'a Course,
}

impl<'a> fmt::Display for CourseItem<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<6} {:<45} {:>8} ({} items)",
            format!("[{}]", self.course.acronym).bold().cyan(),
            self.course.clean_name,
            self.course.total_size_human().dimmed(),
            self.course.materials.len()
        )
    }
}

// Wrapper for Material in Inquire menus
struct MaterialItem<'a> {
    material: &'a Material,
    slide_count: Option<usize>,
}

impl<'a> fmt::Display for MaterialItem<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let icon = self.material.category.icon();
        let size = self.material.size_human();
        let date = self.material.date_modified_human();
        let extra = if let Some(sc) = self.slide_count {
            format!(" [{} slides]", sc)
        } else {
            String::new()
        };

        write!(
            f,
            "{} {:<50} {:>9}{} {:>16}",
            icon,
            truncate_str(&self.material.filename, 50),
            size.dimmed(),
            extra.yellow(),
            date.dimmed()
        )
    }
}

// Wrapper for CanonicalSlide in Inquire menus
struct CanonicalSlideItem<'a> {
    slide: &'a CanonicalSlide<'a>,
}

impl<'a> fmt::Display for CanonicalSlideItem<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lec_badge = format!("[Lec {:02}]", self.slide.lecture_num).bold().magenta();
        let slides_badge = format!("({} slides)", self.slide.slide_count).yellow();
        let name = truncate_str(&self.slide.title, 42);
        let size = self.slide.material.size_human();

        write!(
            f,
            "📑 {} {:<42} {:>11} {:>8}",
            lec_badge,
            name,
            slides_badge,
            size.dimmed()
        )
    }
}

// Wrapper for Search Material Result
struct SearchResultItem<'a> {
    material: &'a Material,
    course_acronym: &'a str,
}

impl<'a> fmt::Display for SearchResultItem<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let badge = format!("[{}]", self.course_acronym).bold().cyan();
        let icon = self.material.category.icon();
        let size = self.material.size_human();
        let name = truncate_str(&self.material.filename, 45);

        write!(
            f,
            "{:<7} {} {:<45} {:>9}",
            badge,
            icon,
            name,
            size.dimmed()
        )
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() > max_len {
        let mut truncated: String = s.chars().take(max_len - 3).collect();
        truncated.push_str("...");
        truncated
    } else {
        s.to_string()
    }
}

pub fn run_interactive(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n{}", "═══ Galgotias University LMS CLI ═══".bold().cyan());

    loop {
        let tracked_count = if store.config.selected_course_ids.is_empty() {
            format!("All ({})", store.courses.len())
        } else {
            format!("{}/{}", store.config.selected_course_ids.len(), store.courses.len())
        };

        let last_sync = store
            .sync_state
            .as_ref()
            .and_then(|s| s.last_sync_human.as_deref())
            .unwrap_or("Never");

        println!(
            "\n{} {} | {} {} | {} {}",
            "Student:".dimmed(),
            store.config.fullname.as_deref().unwrap_or("Not set").bold(),
            "Tracked Courses:".dimmed(),
            tracked_count.cyan(),
            "Last Sync:".dimmed(),
            last_sync.dimmed()
        );

        let options = vec![
            "📚 Browse Courses",
            "📑 View Slides (Deduplicated PPT & Notes)",
            "📝 View Notes & Documents",
            "🔍 Search Materials",
            "⚡ Recent Uploads (Last 7 Days)",
            "🔄 Sync from LMS",
            "⚙️ Configure Tracked Courses",
            "👤 Account & Cache Info",
            "🚪 Exit",
        ];

        let selection = Select::new("Main Menu:", options).prompt();

        match selection {
            Ok("📚 Browse Courses") => browse_courses(store)?,
            Ok("📑 View Slides (Deduplicated PPT & Notes)") => browse_slides_menu(store)?,
            Ok("📝 View Notes & Documents") => browse_notes_menu(store)?,
            Ok("🔍 Search Materials") => search_materials_prompt(store)?,
            Ok("⚡ Recent Uploads (Last 7 Days)") => view_recent_materials(store)?,
            Ok("🔄 Sync from LMS") => run_sync_prompt(store)?,
            Ok("⚙️ Configure Tracked Courses") => configure_courses_prompt(store)?,
            Ok("👤 Account & Cache Info") => show_account_info(store)?,
            Ok("🚪 Exit") | Err(_) => {
                println!("{}", "Goodbye!".green());
                break;
            }
            _ => break,
        }
    }

    Ok(())
}

fn browse_courses(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let mut show_all = false;

    loop {
        let course_list: Vec<&Course> = store.courses(show_all);
        if course_list.is_empty() {
            println!("{}", "No courses found. Try running Sync or showing all courses.".yellow());
            return Ok(());
        }

        let mut choices: Vec<String> = Vec::new();
        let toggle_label = if show_all {
            "🔄 Filter: Showing ALL courses (Click to show Tracked only)".to_string()
        } else {
            "🔄 Filter: Showing TRACKED courses (Click to show All)".to_string()
        };
        choices.push(toggle_label);
        choices.push("⬅️ Back to Main Menu".to_string());

        for c in &course_list {
            choices.push(format!("{}", CourseItem { course: c }));
        }

        let prompt = format!("Select a course ({} available):", course_list.len());
        let selected = Select::new(&prompt, choices).with_page_size(15).prompt();

        match selected {
            Ok(ref s) if s.starts_with("🔄 Filter:") => {
                show_all = !show_all;
            }
            Ok(ref s) if s.starts_with("⬅️ Back") => break,
            Ok(ref s) => {
                // Find selected course
                if let Some(c) = course_list.iter().find(|c| s.contains(&format!("[{}]", c.acronym))) {
                    course_dashboard(store, c)?;
                }
            }
            Err(_) => break,
        }
    }

    Ok(())
}

fn course_dashboard(store: &LmsStore, course: &Course) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        println!("\n{}", "─".repeat(60).dimmed());
        println!(
            "{} {} ({})",
            format!("[{}]", course.acronym).bold().cyan(),
            course.clean_name.bold(),
            course.fullname.dimmed()
        );
        println!(
            "{} items | Total size: {} | Moodle ID: {}",
            course.materials.len().to_string().cyan(),
            course.total_size_human().yellow(),
            course.id.to_string().dimmed()
        );
        println!("{}", "─".repeat(60).dimmed());

        let (canonical, unnumbered) = course.canonical_slides(&store.ppt_meta);
        let slides_count = canonical.len() + unnumbered.len();

        let by_cat = course.by_category();
        let notes_count = by_cat.get(&Category::Notes).map(|v| v.len()).unwrap_or(0);
        let syllabus_count = by_cat.get(&Category::Syllabus).map(|v| v.len()).unwrap_or(0);
        let textbooks_count = by_cat.get(&Category::Textbooks).map(|v| v.len()).unwrap_or(0);

        let options = vec![
            format!("📑 Canonical Slides ({} deduplicated/reviewed)", slides_count),
            format!("📝 Lecture Notes & Documents ({} items)", notes_count),
            format!("📋 Syllabus & Session Plans ({} items)", syllabus_count),
            format!("📚 Textbooks & Reference ({} items)", textbooks_count),
            format!("📦 All Materials ({} items)", course.materials.len()),
            "⬇️ Download Entire Course".to_string(),
            "⬅️ Back to Course List".to_string(),
        ];

        let action = Select::new("Course Options:", options).prompt();

        match action {
            Ok(ref s) if s.starts_with("📑 Canonical Slides") => {
                view_course_slides(store, course)?;
            }
            Ok(ref s) if s.starts_with("📝 Lecture Notes") => {
                view_category_materials(store, course, Category::Notes)?;
            }
            Ok(ref s) if s.starts_with("📋 Syllabus") => {
                view_category_materials(store, course, Category::Syllabus)?;
            }
            Ok(ref s) if s.starts_with("📚 Textbooks") => {
                view_category_materials(store, course, Category::Textbooks)?;
            }
            Ok(ref s) if s.starts_with("📦 All Materials") => {
                view_all_course_materials(store, course)?;
            }
            Ok(ref s) if s.starts_with("⬇️ Download Entire Course") => {
                download_entire_course(store, course)?;
            }
            Ok(ref s) if s.starts_with("⬅️ Back") => break,
            _ => break,
        }
    }

    Ok(())
}

fn view_course_slides(store: &LmsStore, course: &Course) -> Result<(), Box<dyn std::error::Error>> {
    let (canonical, unnumbered) = course.canonical_slides(&store.ppt_meta);

    if canonical.is_empty() && unnumbered.is_empty() {
        println!("{}", "No slides found for this course.".yellow());
        return Ok(());
    }

    let mut choices: Vec<String> = Vec::new();
    choices.push("⬅️ Back".to_string());

    for s in &canonical {
        choices.push(format!("{}", CanonicalSlideItem { slide: s }));
    }
    for m in &unnumbered {
        choices.push(format!("{}", MaterialItem { material: m, slide_count: None }));
    }

    loop {
        let sel = Select::new("Select Slide to Open/Download:", choices.clone())
            .with_page_size(15)
            .prompt();

        match sel {
            Ok(ref s) if s.starts_with("⬅️ Back") => break,
            Ok(ref s) => {
                // Check if canonical
                if let Some(cs) = canonical.iter().find(|cs| s.contains(&format!("[Lec {:02}]", cs.lecture_num))) {
                    let meta = store.get_ppt_meta(cs.material.fileurl.as_deref().unwrap_or_default());
                    handle_material_actions(cs.material, &store.config, meta)?;
                } else if let Some(m) = unnumbered.iter().find(|m| s.contains(&truncate_str(&m.filename, 40))) {
                    let meta = store.get_ppt_meta(m.fileurl.as_deref().unwrap_or_default());
                    handle_material_actions(m, &store.config, meta)?;
                }
            }
            Err(_) => break,
        }
    }

    Ok(())
}

fn view_category_materials(
    store: &LmsStore,
    course: &Course,
    category: Category,
) -> Result<(), Box<dyn std::error::Error>> {
    let by_cat = course.by_category();
    let materials = by_cat.get(&category).cloned().unwrap_or_default();

    if materials.is_empty() {
        println!("{}", format!("No materials found under {}", category.name()).yellow());
        return Ok(());
    }

    let mut choices: Vec<String> = Vec::new();
    choices.push("⬅️ Back".to_string());

    for m in &materials {
        choices.push(format!("{}", MaterialItem { material: m, slide_count: None }));
    }

    loop {
        let sel = Select::new(&format!("{}:", category.name()), choices.clone())
            .with_page_size(15)
            .prompt();

        match sel {
            Ok(ref s) if s.starts_with("⬅️ Back") => break,
            Ok(ref s) => {
                if let Some(m) = materials.iter().find(|m| s.contains(&truncate_str(&m.filename, 40))) {
                    let meta = store.get_ppt_meta(m.fileurl.as_deref().unwrap_or_default());
                    handle_material_actions(m, &store.config, meta)?;
                }
            }
            Err(_) => break,
        }
    }

    Ok(())
}

fn view_all_course_materials(store: &LmsStore, course: &Course) -> Result<(), Box<dyn std::error::Error>> {
    if course.materials.is_empty() {
        println!("{}", "No materials in this course.".yellow());
        return Ok(());
    }

    let mut choices: Vec<String> = Vec::new();
    choices.push("⬅️ Back".to_string());

    for m in &course.materials {
        choices.push(format!("{}", MaterialItem { material: m, slide_count: None }));
    }

    loop {
        let sel = Select::new("Select Material:", choices.clone())
            .with_page_size(15)
            .prompt();

        match sel {
            Ok(ref s) if s.starts_with("⬅️ Back") => break,
            Ok(ref s) => {
                if let Some(m) = course.materials.iter().find(|m| s.contains(&truncate_str(&m.filename, 40))) {
                    let meta = store.get_ppt_meta(m.fileurl.as_deref().unwrap_or_default());
                    handle_material_actions(m, &store.config, meta)?;
                }
            }
            Err(_) => break,
        }
    }

    Ok(())
}

pub fn handle_material_actions(
    material: &Material,
    config: &crate::models::Config,
    ppt_meta: Option<&PptMeta>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        println!("\n{}", "─".repeat(50).dimmed());
        println!("{} {}", "File:".bold(), material.filename.cyan().bold());
        println!("{} {}", "Course:".bold(), material.course_name);
        println!("{} {}", "Category:".bold(), material.category.name());
        println!(
            "{} {} | Modified: {}",
            "Size:".bold(),
            material.size_human().yellow(),
            material.date_modified_human().dimmed()
        );

        if let Some(meta) = ppt_meta {
            if let Some(num) = meta.lecture_num {
                println!("{} Lecture #{}", "Lecture:".bold(), num);
            }
            if let Some(sc) = meta.slide_count {
                println!("{} {} slides", "Slide Count:".bold(), sc);
            }
            if !meta.topics.is_empty() {
                println!("{} {}", "Topics:".bold(), meta.topics.join(", ").dimmed());
            }
        }
        println!("{}", "─".repeat(50).dimmed());

        let actions = vec![
            "🚀 Open (Download if needed, then open)",
            "⬇️ Download",
            "📋 Copy URL with Token to Clipboard",
            "📁 Open Containing Folder",
            "⬅️ Back",
        ];

        let sel = Select::new("Action:", actions).prompt();

        match sel {
            Ok("🚀 Open (Download if needed, then open)") => {
                match download_material(material, config, None, true) {
                    Ok(path) => {
                        println!("{} Opening file: {}", "✓".green(), path.display());
                        if let Err(e) = open_file(&path) {
                            eprintln!("{} Failed to open file: {}", "⚠".yellow(), e);
                        }
                    }
                    Err(e) => eprintln!("{} Download failed: {}", "✗".red(), e),
                }
            }
            Ok("⬇️ Download") => {
                match download_material(material, config, None, true) {
                    Ok(path) => println!("{} Download complete: {}", "✓".green(), path.display()),
                    Err(e) => eprintln!("{} Download failed: {}", "✗".red(), e),
                }
            }
            Ok("📋 Copy URL with Token to Clipboard") => {
                if let Some(ref url) = material.fileurl {
                    if let Some(ref token) = config.token {
                        let full_url = attach_token(url, token);
                        if copy_to_clipboard(&full_url) {
                            println!("{} URL copied to clipboard!", "✓".green().bold());
                        } else {
                            println!("URL: {}", full_url);
                        }
                    }
                } else {
                    println!("{}", "No URL available for this material.".yellow());
                }
            }
            Ok("📁 Open Containing Folder") => {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/faulter"));
                let default_download = config
                    .download_dir
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join("Downloads").join("gulms"));

                let folder = default_download
                    .join(&material.course_acronym)
                    .join(material.category.name());

                let _ = std::fs::create_dir_all(&folder);
                let _ = Command::new("xdg-open").arg(&folder).spawn();
                println!("{} Opened directory: {}", "✓".green(), folder.display());
            }
            Ok("⬅️ Back") | Err(_) => break,
            _ => break,
        }
    }

    Ok(())
}

fn browse_slides_menu(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let courses = store.courses(false);
    let mut options = vec!["⬅️ Back".to_string()];
    for c in &courses {
        let (canonical, unnumbered) = c.canonical_slides(&store.ppt_meta);
        let count = canonical.len() + unnumbered.len();
        options.push(format!("{:<6} {:<45} ({} slides)", format!("[{}]", c.acronym).bold().cyan(), c.clean_name, count));
    }

    let sel = Select::new("Select course for slides:", options).prompt();
    match sel {
        Ok(ref s) if s.starts_with("⬅️ Back") => Ok(()),
        Ok(ref s) => {
            if let Some(c) = courses.iter().find(|c| s.contains(&format!("[{}]", c.acronym))) {
                view_course_slides(store, c)?;
            }
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

fn browse_notes_menu(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let courses = store.courses(false);
    let mut options = vec!["⬅️ Back".to_string()];
    for c in &courses {
        let notes_count = c.by_category().get(&Category::Notes).map(|v| v.len()).unwrap_or(0);
        options.push(format!("{:<6} {:<45} ({} notes)", format!("[{}]", c.acronym).bold().cyan(), c.clean_name, notes_count));
    }

    let sel = Select::new("Select course for notes:", options).prompt();
    match sel {
        Ok(ref s) if s.starts_with("⬅️ Back") => Ok(()),
        Ok(ref s) => {
            if let Some(c) = courses.iter().find(|c| s.contains(&format!("[{}]", c.acronym))) {
                view_category_materials(store, c, Category::Notes)?;
            }
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

fn search_materials_prompt(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let query = Text::new("Search materials (filename, module, topic):").prompt();
    if let Ok(q) = query {
        if q.trim().is_empty() {
            return Ok(());
        }

        let results = store.search_materials(&q, None, true);
        if results.is_empty() {
            println!("{}", format!("No materials found matching '{}'", q).yellow());
            return Ok(());
        }

        let mut choices = vec!["⬅️ Back".to_string()];
        for res in &results {
            choices.push(format!("{}", SearchResultItem {
                material: res.material,
                course_acronym: &res.course.acronym,
            }));
        }

        let sel = Select::new(&format!("Results for '{}' ({} found):", q, results.len()), choices)
            .with_page_size(15)
            .prompt();

        match sel {
            Ok(ref s) if s.starts_with("⬅️ Back") => Ok(()),
            Ok(ref s) => {
                if let Some(res) = results.iter().find(|r| s.contains(&truncate_str(&r.material.filename, 40))) {
                    let meta = store.get_ppt_meta(res.material.fileurl.as_deref().unwrap_or_default());
                    handle_material_actions(res.material, &store.config, meta)?;
                }
                Ok(())
            }
            Err(_) => Ok(()),
        }
    } else {
        Ok(())
    }
}

fn view_recent_materials(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let results = store.recent_materials(7, false);
    if results.is_empty() {
        println!("{}", "No new or modified materials found in the last 7 days.".yellow());
        return Ok(());
    }

    let mut choices = vec!["⬅️ Back".to_string()];
    for res in &results {
        choices.push(format!("{}", SearchResultItem {
            material: res.material,
            course_acronym: &res.course.acronym,
        }));
    }

    let sel = Select::new(&format!("Recent Uploads ({} found):", results.len()), choices)
        .with_page_size(15)
        .prompt();

    match sel {
        Ok(ref s) if s.starts_with("⬅️ Back") => Ok(()),
        Ok(ref s) => {
            if let Some(res) = results.iter().find(|r| s.contains(&truncate_str(&r.material.filename, 40))) {
                let meta = store.get_ppt_meta(res.material.fileurl.as_deref().unwrap_or_default());
                handle_material_actions(res.material, &store.config, meta)?;
            }
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

fn download_entire_course(store: &LmsStore, course: &Course) -> Result<(), Box<dyn std::error::Error>> {
    let proceed = Confirm::new(&format!(
        "Download all {} materials ({}) for {}?",
        course.materials.len(),
        course.total_size_human(),
        course.clean_name
    ))
    .with_default(true)
    .prompt();

    if let Ok(true) = proceed {
        println!("\n{} Downloading all materials for {}...", "⬇".cyan().bold(), course.clean_name);
        let mut downloaded = 0;
        let mut failed = 0;

        for m in &course.materials {
            if m.fileurl.is_none() {
                continue;
            }
            match download_material(m, &store.config, None, true) {
                Ok(_) => downloaded += 1,
                Err(e) => {
                    eprintln!("  ⚠ Error downloading {}: {}", m.filename, e);
                    failed += 1;
                }
            }
        }

        println!(
            "\n{} Finished course download: {} succeeded, {} failed.",
            "✓".green().bold(),
            downloaded,
            failed
        );
    }

    Ok(())
}

fn run_sync_prompt(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let mode = Select::new(
        "Sync Mode:",
        vec![
            "⚡ Incremental Delta Sync (Fast, only checks modified courses)",
            "🔄 Full Force Sync (Re-fetches contents for all courses)",
            "⬅️ Back",
        ],
    )
    .prompt();

    match mode {
        Ok("⚡ Incremental Delta Sync (Fast, only checks modified courses)") => {
            let mut client = LmsClient::new(
                store.config.base_url.clone(),
                store.config.token.clone(),
                store.config.user_id,
            );
            println!("\n{} Running Incremental Delta Sync...", "🔄".cyan().bold());
            match client.sync(store, false) {
                Ok(stats) => {
                    println!(
                        "\n{} Sync completed in {:.2}s!",
                        "✓".green().bold(),
                        stats.elapsed.as_secs_f64()
                    );
                    println!(
                        "  Checked: {} courses | Updated: {} courses | New files: {}",
                        stats.checked_count,
                        stats.updated_courses.len(),
                        stats.new_files_count
                    );
                }
                Err(e) => eprintln!("{} Sync failed: {}", "✗".red().bold(), e),
            }
        }
        Ok("🔄 Full Force Sync (Re-fetches contents for all courses)") => {
            let mut client = LmsClient::new(
                store.config.base_url.clone(),
                store.config.token.clone(),
                store.config.user_id,
            );
            println!("\n{} Running Full Force Sync...", "🔄".cyan().bold());
            match client.sync(store, true) {
                Ok(stats) => {
                    println!(
                        "\n{} Force sync completed in {:.2}s!",
                        "✓".green().bold(),
                        stats.elapsed.as_secs_f64()
                    );
                    println!(
                        "  Updated all {} courses ({} new files detected).",
                        stats.updated_courses.len(),
                        stats.new_files_count
                    );
                }
                Err(e) => eprintln!("{} Sync failed: {}", "✗".red().bold(), e),
            }
        }
        _ => {}
    }

    Ok(())
}

fn configure_courses_prompt(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let all = store.all_courses();
    if all.is_empty() {
        println!("{}", "No courses found to configure.".yellow());
        return Ok(());
    }

    let items: Vec<String> = all
        .iter()
        .map(|c| format!("{:<6} {}", format!("[{}]", c.acronym), c.clean_name))
        .collect();

    let default_indices: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, c)| store.config.selected_course_ids.contains(&c.id))
        .map(|(i, _)| i)
        .collect();

    let sel = MultiSelect::new("Select courses to actively track (Space to toggle, Enter to confirm):", items)
        .with_default(&default_indices)
        .prompt();

    if let Ok(chosen) = sel {
        let mut new_ids = Vec::new();
        for item in chosen {
            if let Some(c) = all.iter().find(|c| item.contains(&format!("[{}]", c.acronym))) {
                new_ids.push(c.id);
            }
        }

        store.save_selected_courses(new_ids)?;
        println!(
            "{} Updated active course tracking! ({} selected)",
            "✓".green().bold(),
            store.config.selected_course_ids.len()
        );
    }

    Ok(())
}

fn show_account_info(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n{}", "═══ GULMS Account & System Info ═══".bold().cyan());
    println!("  {:<18} {}", "Base Portal:".bold(), store.config.base_url);
    println!("  {:<18} {}", "Student Name:".bold(), store.config.fullname.as_deref().unwrap_or("Not set"));
    println!("  {:<18} {}", "Username:".bold(), store.config.username.as_deref().unwrap_or("Not set"));
    println!("  {:<18} {}", "User ID:".bold(), store.config.user_id.map(|u| u.to_string()).unwrap_or_else(|| "Not set".to_string()));
    println!("  {:<18} {}", "Token Configured:".bold(), if store.config.token.is_some() { "Yes".green() } else { "No".red() });
    println!("  {:<18} {}", "Download Directory:".bold(), store.download_dir().display().to_string().cyan());
    println!("  {:<18} {}", "Config File:".bold(), store.config_path.display());
    println!("  {:<18} {}", "Courses Cache:".bold(), store.courses_cache_path.display());
    println!("  {:<18} {}", "PPT Metadata:".bold(), store.ppt_meta_path.display());
    println!("  {:<18} {} courses cached", "Cached Courses:".bold(), store.courses.len());

    let _ = Text::new("Press Enter to return to main menu...").prompt();
    Ok(())
}
