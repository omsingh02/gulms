use colored::*;
use inquire::{MultiSelect, Select, Text};
use std::fmt;

use crate::categorizer::Category;
use crate::downloader::{attach_token, download_material, material_dir};
use crate::lectures::{self, ExportOptions, Mode, ensure_analysed, export_course};
use crate::models::{CanonicalSlide, Course, Material, PptMeta};
use crate::platform::{copy_to_clipboard, open_file, open_folder};
use crate::store::LmsStore;
use crate::sync::sync_and_report;
use crate::ui::{BIN, badge, badge_width, print_account_info, truncate};

const BACK: &str = "⬅️ Back";

/// Show `labels` under a leading "Back" entry. Returns the index into `labels`
/// of the chosen row, or `None` for Back / Esc / Ctrl-C.
///
/// Selecting by index (rather than re-matching the rendered label) stays correct
/// when labels are truncated or two rows look alike.
fn select_index(prompt: &str, labels: Vec<String>) -> Option<usize> {
    let mut choices = Vec::with_capacity(labels.len() + 1);
    choices.push(BACK.to_string());
    choices.extend(labels);

    match Select::new(prompt, choices).with_page_size(15).raw_prompt() {
        Ok(opt) if opt.index > 0 => Some(opt.index - 1),
        _ => None,
    }
}

// Wrapper for Course in Inquire menus
struct CourseItem<'a> {
    course: &'a Course,
    badge_width: usize,
}

impl<'a> fmt::Display for CourseItem<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {:<45} {:>8} ({} items)",
            badge(&self.course.acronym, self.badge_width),
            truncate(&self.course.clean_name, 45),
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
            truncate(&self.material.filename, 50),
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
        let lec_badge = format!("[Lec {:02}]", self.slide.lecture_num)
            .bold()
            .magenta();
        let slides_badge = format!("({} slides)", self.slide.slide_count).yellow();
        let name = truncate(&self.slide.title, 42);
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
        let name = truncate(&self.material.filename, 45);

        write!(f, "{:<7} {} {:<45} {:>9}", badge, icon, name, size.dimmed())
    }
}

pub fn run_interactive(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n{}", "═══ gulms ═══".bold().cyan());

    if store.config.token.is_none() {
        println!(
            "{}",
            "Not signed in. Run `gulms login` to get started.".yellow()
        );
    } else if store.courses.is_empty() {
        println!(
            "{}",
            "No courses cached yet. Choose \"Sync from LMS\" to fetch them.".yellow()
        );
    }

    loop {
        let tracked_count = if store.config.selected_course_ids.is_empty() {
            format!("All ({})", store.courses.len())
        } else {
            format!(
                "{}/{}",
                store.config.selected_course_ids.len(),
                store.courses.len()
            )
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

pub fn browse_courses(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let mut show_all = false;

    loop {
        let course_list: Vec<&Course> = store.courses(show_all);
        if course_list.is_empty() {
            println!(
                "{}",
                format!(
                    "No courses found. Try `{} sync` or showing all courses.",
                    BIN
                )
                .yellow()
            );
            if show_all {
                return Ok(());
            }
        }

        let width = badge_width(course_list.iter().map(|c| c.acronym.as_str()));
        let mut choices: Vec<String> = vec![
            if show_all {
                "🔄 Filter: Showing ALL courses (select to show Tracked only)".to_string()
            } else {
                "🔄 Filter: Showing TRACKED courses (select to show All)".to_string()
            },
            "⬅️ Back to Main Menu".to_string(),
        ];
        choices.extend(course_list.iter().map(|c| {
            CourseItem {
                course: c,
                badge_width: width,
            }
            .to_string()
        }));

        let prompt = format!("Select a course ({} available):", course_list.len());
        match Select::new(&prompt, choices)
            .with_page_size(15)
            .raw_prompt()
        {
            Ok(opt) => match opt.index {
                0 => show_all = !show_all,
                1 => break,
                i => {
                    let id = course_list[i - 2].id;
                    course_dashboard(store, id)?
                }
            },
            Err(_) => break,
        }
    }

    Ok(())
}

fn course_dashboard(
    store: &mut LmsStore,
    course_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        // Re-read each time: analysis and sync change what the course looks like.
        let Some(course) = store.courses.iter().find(|c| c.id == course_id).cloned() else {
            return Ok(());
        };

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
        let lectures_count = canonical.len();

        let by_cat = course.by_category();
        let notes_count = by_cat.get(&Category::Notes).map(|v| v.len()).unwrap_or(0);
        let syllabus_count = by_cat
            .get(&Category::Syllabus)
            .map(|v| v.len())
            .unwrap_or(0);
        let textbooks_count = by_cat
            .get(&Category::Textbooks)
            .map(|v| v.len())
            .unwrap_or(0);

        let options = vec![
            format!(
                "📑 Lecture Slides ({} lectures, {} files)",
                lectures_count, slides_count
            ),
            format!("📝 Lecture Notes & Documents ({} items)", notes_count),
            format!("📋 Syllabus & Session Plans ({} items)", syllabus_count),
            format!("📚 Textbooks & Reference ({} items)", textbooks_count),
            format!("📦 All Materials ({} items)", course.materials.len()),
            "🧾 Export Study Notes (Markdown + PDF)".to_string(),
            "⬇️ Download Entire Course".to_string(),
            "⬅️ Back to Course List".to_string(),
        ];

        // Keep this order in sync with the match arms below.
        let action = Select::new("Course Options:", options).raw_prompt();

        match action.map(|opt| opt.index) {
            Ok(0) => {
                ensure_analysed(store, &[course_id], Mode::Ask);
                let fresh = store
                    .courses
                    .iter()
                    .find(|c| c.id == course_id)
                    .cloned()
                    .unwrap_or(course);
                view_course_slides(store, &fresh)?
            }
            Ok(1) => view_category_materials(store, &course, Category::Notes)?,
            Ok(2) => view_category_materials(store, &course, Category::Syllabus)?,
            Ok(3) => view_category_materials(store, &course, Category::Textbooks)?,
            Ok(4) => {
                view_material_list(store, "Select Material:", course.materials.iter().collect())?
            }
            Ok(5) => export_notes_prompt(store, course_id)?,
            Ok(6) => {
                if let Err(e) = lectures::download_course(store, course_id, None, false) {
                    eprintln!("{} {}", "✗".red(), e);
                }
            }
            _ => break,
        }
    }

    Ok(())
}

/// Pick one lecture (or all) and export study notes for it.
fn export_notes_prompt(
    store: &mut LmsStore,
    course_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_analysed(store, &[course_id], Mode::Ask);

    let Some(course) = store.courses.iter().find(|c| c.id == course_id) else {
        return Ok(());
    };
    let (canonical, _) = course.canonical_slides(&store.ppt_meta);
    if canonical.is_empty() {
        println!("{}", "No numbered lectures to export yet.".yellow());
        return Ok(());
    }

    let mut labels = vec![format!("All {} lectures", canonical.len())];
    labels.extend(canonical.iter().map(|c| {
        format!(
            "Lec {:02} - {} ({} slides)",
            c.lecture_num,
            truncate(&c.title, 50),
            c.slide_count
        )
    }));
    let numbers: Vec<u32> = canonical.iter().map(|c| c.lecture_num).collect();

    let Some(pick) = select_index("Export which lecture?", labels) else {
        return Ok(());
    };
    let lecture = if pick == 0 {
        None
    } else {
        Some(numbers[pick - 1])
    };

    let result = export_course(
        store,
        course_id,
        &ExportOptions {
            lecture,
            dest: None,
            pdf: true,
            open: lecture.is_some(),
            ocr: true,
        },
    );
    if let Err(e) = result {
        eprintln!("{} {}", "✗".red(), e);
    }
    Ok(())
}

fn open_material(store: &LmsStore, material: &Material) -> Result<(), Box<dyn std::error::Error>> {
    let meta = store.get_ppt_meta(material.fileurl.as_deref().unwrap_or_default());
    handle_material_actions(material, &store.config, meta)
}

/// Pick-a-material loop shared by every flat file list; returns on Back/Esc.
fn view_material_list(
    store: &LmsStore,
    prompt: &str,
    materials: Vec<&Material>,
) -> Result<(), Box<dyn std::error::Error>> {
    if materials.is_empty() {
        println!("{}", "No materials to show.".yellow());
        return Ok(());
    }

    let labels: Vec<String> = materials
        .iter()
        .map(|m| {
            MaterialItem {
                material: m,
                slide_count: None,
            }
            .to_string()
        })
        .collect();

    while let Some(i) = select_index(prompt, labels.clone()) {
        open_material(store, materials[i])?;
    }

    Ok(())
}

fn view_course_slides(store: &LmsStore, course: &Course) -> Result<(), Box<dyn std::error::Error>> {
    let (canonical, unnumbered) = course.canonical_slides(&store.ppt_meta);

    if canonical.is_empty() && unnumbered.is_empty() {
        println!("{}", "No slides found for this course.".yellow());
        return Ok(());
    }

    // Canonical (numbered) slides first, then the unnumbered extras; `materials`
    // mirrors `labels` one-to-one so the chosen index maps straight back.
    let mut labels: Vec<String> = Vec::new();
    let mut materials: Vec<&Material> = Vec::new();
    for s in &canonical {
        labels.push(CanonicalSlideItem { slide: s }.to_string());
        materials.push(s.material);
    }
    for m in &unnumbered {
        labels.push(
            MaterialItem {
                material: m,
                slide_count: None,
            }
            .to_string(),
        );
        materials.push(m);
    }

    while let Some(i) = select_index("Select Slide to Open/Download:", labels.clone()) {
        open_material(store, materials[i])?;
    }

    Ok(())
}

fn view_category_materials(
    store: &LmsStore,
    course: &Course,
    category: Category,
) -> Result<(), Box<dyn std::error::Error>> {
    let materials: Vec<&Material> = course
        .materials
        .iter()
        .filter(|m| m.category == category)
        .collect();

    if materials.is_empty() {
        println!(
            "{}",
            format!("No materials found under {}", category.name()).yellow()
        );
        return Ok(());
    }

    view_material_list(store, &format!("{}:", category.name()), materials)
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
                match download_material(material, config, None, true, None) {
                    Ok(path) => {
                        println!("{} Opening file: {}", "✓".green(), path.display());
                        if let Err(e) = open_file(&path, config) {
                            eprintln!("{} Failed to open file: {}", "⚠".yellow(), e);
                        }
                    }
                    Err(e) => eprintln!("{} Download failed: {}", "✗".red(), e),
                }
            }
            Ok("⬇️ Download") => match download_material(material, config, None, true, None) {
                Ok(path) => println!("{} Download complete: {}", "✓".green(), path.display()),
                Err(e) => eprintln!("{} Download failed: {}", "✗".red(), e),
            },
            Ok("📋 Copy URL with Token to Clipboard") => {
                match (material.fileurl.as_deref(), config.token.as_deref()) {
                    (Some(url), Some(token)) => {
                        let full_url = attach_token(url, token);
                        if copy_to_clipboard(&full_url) {
                            println!("{} URL copied to clipboard!", "✓".green().bold());
                        } else {
                            println!(
                                "{} No clipboard tool (wl-copy/xclip) found. URL:\n{}",
                                "⚠".yellow(),
                                full_url
                            );
                        }
                    }
                    (None, _) => println!("{}", "No URL available for this material.".yellow()),
                    (_, None) => println!("{}", "Not signed in. Run `gulms login` first.".yellow()),
                }
            }
            Ok("📁 Open Containing Folder") => {
                let folder = material_dir(material, config);
                let opened = std::fs::create_dir_all(&folder)
                    .map_err(|e| e.to_string())
                    .and_then(|_| open_folder(&folder).map_err(|e| e.to_string()));
                match opened {
                    Ok(()) => println!("{} Opened directory: {}", "✓".green(), folder.display()),
                    Err(e) => eprintln!("{} Failed to open {}: {}", "✗".red(), folder.display(), e),
                }
            }
            Ok("⬅️ Back") | Err(_) => break,
            _ => break,
        }
    }

    Ok(())
}

/// Course picker that loops until Back, so returning from a course lands on the
/// course list again rather than dumping the user at the main menu.
fn pick_course_loop(
    store: &LmsStore,
    prompt: &str,
    describe: impl Fn(&Course) -> String,
    mut open: impl FnMut(&Course) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let courses = store.courses(false);
    if courses.is_empty() {
        println!(
            "{}",
            format!("No courses found. Run `{} sync` first.", BIN).yellow()
        );
        return Ok(());
    }

    let width = badge_width(courses.iter().map(|c| c.acronym.as_str()));
    let labels: Vec<String> = courses
        .iter()
        .map(|c| {
            format!(
                "{} {:<45} ({})",
                badge(&c.acronym, width),
                truncate(&c.clean_name, 45),
                describe(c)
            )
        })
        .collect();

    while let Some(i) = select_index(prompt, labels.clone()) {
        open(courses[i])?;
    }

    Ok(())
}

pub fn browse_slides_menu(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let (ids, labels): (Vec<u64>, Vec<String>) = {
            let courses = store.courses(false);
            if courses.is_empty() {
                println!(
                    "{}",
                    format!("No courses found. Run `{} sync` first.", BIN).yellow()
                );
                return Ok(());
            }
            let width = badge_width(courses.iter().map(|c| c.acronym.as_str()));
            courses
                .iter()
                .map(|c| {
                    let (canonical, unnumbered) = c.canonical_slides(&store.ppt_meta);
                    (
                        c.id,
                        format!(
                            "{} {:<45} ({} lectures, {} files)",
                            badge(&c.acronym, width),
                            truncate(&c.clean_name, 45),
                            canonical.len(),
                            canonical.len() + unnumbered.len()
                        ),
                    )
                })
                .unzip()
        };

        let Some(i) = select_index("Select course for slides:", labels) else {
            return Ok(());
        };
        ensure_analysed(store, &[ids[i]], Mode::Ask);
        if let Some(course) = store.courses.iter().find(|c| c.id == ids[i]) {
            view_course_slides(store, course)?;
        }
    }
}

pub fn browse_notes_menu(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    pick_course_loop(
        store,
        "Select course for notes:",
        |c| {
            let notes = c
                .materials
                .iter()
                .filter(|m| m.category == Category::Notes)
                .count();
            format!("{} notes", notes)
        },
        |c| view_category_materials(store, c, Category::Notes),
    )
}

/// Results list for search / recent uploads; loops until Back so the user can
/// open several files without re-running the query.
fn view_result_list(
    store: &LmsStore,
    prompt: &str,
    results: &[crate::store::MaterialResult],
) -> Result<(), Box<dyn std::error::Error>> {
    let labels: Vec<String> = results
        .iter()
        .map(|res| {
            SearchResultItem {
                material: res.material,
                course_acronym: &res.course.acronym,
            }
            .to_string()
        })
        .collect();

    while let Some(i) = select_index(prompt, labels.clone()) {
        open_material(store, results[i].material)?;
    }

    Ok(())
}

fn search_materials_prompt(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let Ok(q) = Text::new("Search materials (filename, module, topic):").prompt() else {
        return Ok(());
    };
    if q.trim().is_empty() {
        return Ok(());
    }

    let results = store.search_materials(&q, None, true);
    if results.is_empty() {
        println!(
            "{}",
            format!("No materials found matching '{}'", q).yellow()
        );
        return Ok(());
    }

    view_result_list(
        store,
        &format!("Results for '{}' ({} found):", q.trim(), results.len()),
        &results,
    )
}

fn view_recent_materials(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let results = store.recent_materials(7, false);
    if results.is_empty() {
        println!(
            "{}",
            "No new or modified materials found in the last 7 days.".yellow()
        );
        return Ok(());
    }

    view_result_list(
        store,
        &format!("Recent Uploads ({} found):", results.len()),
        &results,
    )
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
            sync_and_report(store, false);
        }
        Ok("🔄 Full Force Sync (Re-fetches contents for all courses)") => {
            sync_and_report(store, true);
        }
        _ => {}
    }

    Ok(())
}

pub fn configure_courses_prompt(store: &mut LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    let all = store.all_courses();
    if all.is_empty() {
        println!(
            "{}",
            format!("No courses found to configure. Run `{} sync` first.", BIN).yellow()
        );
        return Ok(());
    }

    let width = badge_width(all.iter().map(|c| c.acronym.as_str()));
    let items: Vec<String> = all
        .iter()
        .map(|c| format!("{} {}", badge(&c.acronym, width), c.clean_name))
        .collect();

    let default_indices: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, c)| store.config.selected_course_ids.contains(&c.id))
        .map(|(i, _)| i)
        .collect();

    let sel = MultiSelect::new(
        "Select courses to actively track (Space to toggle, Enter to confirm, Esc to cancel):",
        items,
    )
    .with_default(&default_indices)
    .with_page_size(15)
    .raw_prompt();

    // Esc / Ctrl-C leaves the current selection untouched.
    if let Ok(chosen) = sel {
        let new_ids: Vec<u64> = chosen.iter().map(|opt| all[opt.index].id).collect();

        store.save_selected_courses(new_ids)?;
        let tracked = store.config.selected_course_ids.len();
        if tracked == 0 {
            println!("{} Tracking all courses.", "✓".green().bold());
        } else {
            println!(
                "{} Updated active course tracking! ({} selected)",
                "✓".green().bold(),
                tracked
            );
        }
    }

    Ok(())
}

fn show_account_info(store: &LmsStore) -> Result<(), Box<dyn std::error::Error>> {
    print_account_info(store);
    let _ = Text::new("Press Enter to return to main menu...").prompt();
    Ok(())
}
