//! `gulms doctor`: checks the things that usually go wrong and says how to fix them.

use colored::*;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{http, render, slide_extractor, store::LmsStore, sync::LmsClient, ui::BIN};

#[derive(Default)]
struct Report {
    warnings: usize,
    failures: usize,
}

impl Report {
    fn ok(&self, what: &str, detail: &str) {
        println!("  {} {:<22} {}", "✓".green().bold(), what, detail);
    }

    fn warn(&mut self, what: &str, detail: &str, fix: &str) {
        self.warnings += 1;
        println!("  {} {:<22} {}", "!".yellow().bold(), what, detail);
        println!("    {} {}", "→".dimmed(), fix.dimmed());
    }

    fn fail(&mut self, what: &str, detail: &str, fix: &str) {
        self.failures += 1;
        println!("  {} {:<22} {}", "✗".red().bold(), what, detail);
        println!("    {} {}", "→".dimmed(), fix.dimmed());
    }
}

/// Is the portal's web server answering at all? Any HTTP status counts as yes.
fn portal_reachable(base_url: &str) -> Result<(), String> {
    match http::agent().get(base_url).call() {
        Ok(_) | Err(ureq::Error::StatusCode(_)) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Run every check and print the result. Returns false when something is broken
/// (missing optional tools only warn).
pub fn run(store: &LmsStore, offline: bool) -> bool {
    let mut r = Report::default();
    println!(
        "\n{} {} {}\n",
        "═══".cyan().bold(),
        format!("{BIN} doctor").bold().cyan(),
        "═══".cyan().bold()
    );
    r.ok(
        "Version",
        &format!(
            "{} {} on {}/{}",
            BIN,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );

    // Config file
    let config_path = store.config_path.display().to_string();
    if !store.config_path.exists() {
        r.warn(
            "Config",
            "not created yet",
            &format!("run `{BIN} setup` to sign in and choose your courses"),
        );
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&store.config_path)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0o600);
            if store.config.token.is_some() && mode & 0o077 != 0 {
                r.warn(
                    "Config",
                    &format!("{config_path} is readable by other users"),
                    &format!("run `chmod 600 {config_path}` (it holds your login token)"),
                );
            } else {
                r.ok("Config", &config_path);
            }
        }
        #[cfg(not(unix))]
        r.ok("Config", &config_path);
    }

    // Sign-in and portal
    let base = &store.config.base_url;
    if store.config.token.is_none() {
        r.fail("Signed in", "no", &format!("run `{BIN} login`"));
        if !offline {
            match portal_reachable(base) {
                Ok(()) => r.ok("Portal", &format!("{base} is reachable")),
                Err(e) => r.fail(
                    "Portal",
                    &format!("cannot reach {base} ({e})"),
                    "check your internet connection and the portal address",
                ),
            }
        }
    } else if offline {
        r.ok("Signed in", "token saved (not verified: --offline)");
    } else {
        match LmsClient::from_store(store).get_site_info() {
            Ok(info) => {
                r.ok("Portal", &format!("{base} is reachable"));
                r.ok(
                    "Signed in",
                    &format!("as {}", info.fullname.as_deref().unwrap_or("unknown user")),
                );
            }
            Err(e) => {
                let message = e.to_string();
                if message.contains("session has expired") {
                    r.ok("Portal", &format!("{base} is reachable"));
                    r.fail(
                        "Signed in",
                        "your session expired",
                        &format!("run `{BIN} login`"),
                    );
                } else {
                    r.fail(
                        "Portal",
                        &format!("cannot reach {base} ({message})"),
                        "check your internet connection and the portal address",
                    );
                }
            }
        }
    }

    // Local data
    if store.courses.is_empty() {
        r.warn("Courses", "none cached", &format!("run `{BIN} sync`"));
    } else {
        let age_days = store
            .sync_state
            .as_ref()
            .and_then(|s| s.last_sync)
            .and_then(|t| {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
                Some(now.saturating_sub(t) / 86_400)
            });
        let detail = format!(
            "{} cached, last synced {}",
            store.courses.len(),
            match age_days {
                Some(0) => "today".to_string(),
                Some(1) => "yesterday".to_string(),
                Some(d) => format!("{d} days ago"),
                None => "never".to_string(),
            }
        );
        if age_days.is_none_or(|d| d > 7) {
            r.warn("Courses", &detail, &format!("run `{BIN} sync` to catch up"));
        } else {
            r.ok("Courses", &detail);
        }

        let tracked: Vec<u64> = store.courses(false).iter().map(|c| c.id).collect();
        let decks: usize = store
            .courses
            .iter()
            .filter(|c| tracked.contains(&c.id))
            .flat_map(|c| c.materials.iter())
            .filter(|m| m.is_ppt())
            .count();
        let pending = crate::lectures::pending_jobs(store, &tracked).len();
        if decks > 0 && pending > 0 {
            r.warn(
                "Lectures",
                &format!("{pending} of {decks} slide decks not analysed"),
                &format!("run `{BIN} analyze` so lectures can be numbered and titled"),
            );
        } else if decks > 0 {
            r.ok("Lectures", &format!("all {decks} slide decks analysed"));
        }
    }

    // Download folder
    let dir = store.download_dir();
    let probe = dir.join(format!(".{BIN}-write-test"));
    match std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&probe, b""))
        .and_then(|_| std::fs::remove_file(&probe))
    {
        Ok(()) => r.ok("Download folder", &dir.display().to_string()),
        Err(e) => r.fail(
            "Download folder",
            &format!("{} is not writable ({e})", dir.display()),
            "pick another folder with `setup`, or set \"download_dir\" in config.json",
        ),
    }

    // Optional tools
    match render::find_browser() {
        Some(path) => r.ok("PDF browser", &path.display().to_string()),
        None => r.warn(
            "PDF browser",
            "no Chrome, Chromium, Edge or Brave found",
            "install one for PDF study notes, or set CHROME_BIN (Markdown export works without it)",
        ),
    }
    if slide_extractor::tesseract_available() {
        r.ok("OCR (tesseract)", "found");
    } else {
        r.warn(
            "OCR (tesseract)",
            "not installed (optional)",
            "install tesseract to read text inside screenshots and figures",
        );
    }

    println!();
    match (r.failures, r.warnings) {
        (0, 0) => println!("{} Everything looks good.", "✓".green().bold()),
        (0, w) => println!(
            "{} Working, with {} thing{} worth a look.",
            "✓".green().bold(),
            w,
            if w == 1 { "" } else { "s" }
        ),
        (f, _) => println!(
            "{} {} problem{} to fix, see the arrows above.",
            "✗".red().bold(),
            f,
            if f == 1 { "" } else { "s" }
        ),
    }
    println!();
    r.failures == 0
}
