//! Sign-in, sign-out and the first-run setup wizard.
//!
//! The password is only ever sent to the portal's `/login/token.php` endpoint to
//! obtain a Moodle mobile token; it is never written to disk.

use colored::*;
use inquire::{Password, PasswordDisplayMode, Text};
use serde_json::Value;
use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;

use crate::categorizer::clean_text;
use crate::http;
use crate::interactive;
use crate::store::LmsStore;
use crate::sync::{LmsClient, sync_and_report};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Default)]
pub struct LoginArgs {
    pub username: Option<String>,
    pub url: Option<String>,
    pub password_stdin: bool,
}

/// Turn what a user typed (`lms.example.edu`, `https://lms.example.edu/`) into a
/// canonical base URL without a trailing slash.
pub fn normalize_base_url(input: &str) -> std::result::Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("the portal URL is empty".to_string());
    }
    let url = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed)
    };
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("'{}' is not an http(s) URL", input.trim()));
    }
    Ok(url)
}

fn is_insecure_remote(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', ':']).next().unwrap_or_default();
    !(host == "localhost" || host == "127.0.0.1" || host == "[::1]")
}

/// Exchange username + password for a Moodle mobile token.
pub fn request_token(base_url: &str, username: &str, password: &str) -> Result<String> {
    let url = format!("{}/login/token.php", base_url.trim_end_matches('/'));
    let mut resp = http::agent()
        .post(&url)
        .header("Accept", "application/json")
        .send_form([
            ("username", username),
            ("password", password),
            ("service", "moodle_mobile_app"),
        ])
        .map_err(|e| {
            format!(
                "could not reach {}: {} (check your internet connection)",
                base_url, e
            )
        })?;

    let body: Value = resp
        .body_mut()
        .read_json()
        .map_err(|_| format!("{} did not look like a Moodle portal", base_url))?;

    match body.get("token").and_then(Value::as_str) {
        Some(token) => Ok(token.to_string()),
        None => {
            let reason = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unexpected response from the portal");
            Err(format!("sign-in failed: {}", reason).into())
        }
    }
}

fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn read_password_from_stdin() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        return Err("no password received on stdin".into());
    }
    Ok(password)
}

/// Sign in and save the token. Prompts for whatever isn't supplied when run in a
/// terminal; in scripts pass `--username` and `--password-stdin`.
pub fn login(store: &mut LmsStore, args: LoginArgs) -> Result<()> {
    let previous_url = store.config.base_url.clone();
    let previous_user = store.config.user_id;

    if let Some(url) = args.url.as_deref() {
        store.config.base_url = normalize_base_url(url)?;
    }
    if is_insecure_remote(&store.config.base_url) {
        eprintln!(
            "{} {} is not HTTPS; your password would be sent unencrypted.",
            "warning:".yellow().bold(),
            store.config.base_url
        );
    }

    let interactive = stdin_is_terminal();

    let username = match args.username {
        Some(u) => u,
        None if interactive => {
            let mut prompt = Text::new("Username / admission number:");
            if let Some(existing) = store.config.username.as_deref() {
                prompt = prompt.with_default(existing);
            }
            prompt.prompt()?
        }
        None => return Err("no terminal available; pass --username".into()),
    };
    let username = username.trim().to_string();
    if username.is_empty() {
        return Err("username cannot be empty".into());
    }

    let password = if args.password_stdin {
        read_password_from_stdin()?
    } else if interactive {
        Password::new("Password:")
            .without_confirmation()
            .with_display_mode(PasswordDisplayMode::Masked)
            .prompt()?
    } else {
        return Err("no terminal available; pass --password-stdin and pipe the password in".into());
    };

    println!("{} Signing in to {}...", "•".cyan(), store.config.base_url);
    let token = request_token(&store.config.base_url, &username, &password)?;
    drop(password);

    let info =
        LmsClient::new(store.config.base_url.clone(), Some(token.clone()), None).get_site_info();

    store.config.token = Some(token);
    store.config.username = Some(username);
    match info {
        Ok(info) => {
            store.config.user_id = info.userid;
            store.config.fullname = info.fullname.map(|n| clean_text(&n));
        }
        Err(e) => eprintln!(
            "{} signed in, but could not read your profile: {}",
            "warning:".yellow().bold(),
            e
        ),
    }

    // Cached courses and the tracked-course list belong to the previous account.
    let new_user = store.config.user_id;
    let account_changed = previous_url != store.config.base_url
        || matches!((previous_user, new_user), (Some(a), Some(b)) if a != b);
    if account_changed {
        store.clear_account_data();
    }

    store.save_config()?;

    println!(
        "{} Signed in as {}",
        "✓".green().bold(),
        store
            .config
            .fullname
            .as_deref()
            .or(store.config.username.as_deref())
            .unwrap_or("unknown user")
            .bold()
    );
    Ok(())
}

/// Remove the saved token. Cached course data is kept.
pub fn logout(store: &mut LmsStore) -> Result<()> {
    if store.config.token.take().is_none() {
        println!("Already signed out.");
        return Ok(());
    }
    store.save_config()?;
    println!(
        "{} Signed out; the token was removed from {}.",
        "✓".green().bold(),
        store.config_path.display()
    );
    println!(
        "{}",
        "Cached course data was kept. The token itself stays valid on the portal until you reset it under Preferences → Security keys."
            .dimmed()
    );
    Ok(())
}

fn expand_home(input: &str) -> PathBuf {
    if input == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(input));
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(input)
}

/// First-run wizard: sign in, fetch courses, choose which to track, pick a
/// download folder. Safe to re-run at any time.
pub fn run_setup(store: &mut LmsStore) -> Result<()> {
    if !stdin_is_terminal() {
        return Err(
            "`setup` needs an interactive terminal (use `login --password-stdin` in scripts)"
                .into(),
        );
    }

    println!("\n{}", "═══ Welcome to gulms ═══".bold().cyan());
    println!(
        "{}",
        "Sign in, pick your courses, and choose where files are saved.".dimmed()
    );

    // 1. Sign in (reuse the saved session if it still works)
    println!("\n{} Sign in", "[1/3]".bold());
    let existing_session = store
        .config
        .token
        .is_some()
        .then(|| LmsClient::from_store(store).get_site_info());
    match existing_session {
        Some(Ok(info)) => println!(
            "{} Already signed in as {}",
            "✓".green().bold(),
            info.fullname.as_deref().unwrap_or("unknown user")
        ),
        Some(Err(e)) => {
            println!(
                "{} Saved session is no longer valid ({}).",
                "!".yellow().bold(),
                e
            );
            login(store, LoginArgs::default())?;
        }
        None => login(store, LoginArgs::default())?,
    }

    // 2. Courses
    println!("\n{} Your courses", "[2/3]".bold());
    if !sync_and_report(store, false) {
        return Err(
            "could not fetch your courses; fix the problem above and run `gulms setup` again"
                .into(),
        );
    }
    println!(
        "{}",
        "Choose the courses you want to track (leave all unticked to track everything).".dimmed()
    );
    interactive::configure_courses_prompt(store)?;

    // 3. Download folder
    println!("\n{} Download folder", "[3/3]".bold());
    let default_dir = store.download_dir().display().to_string();
    let answer = Text::new("Save downloaded files to:")
        .with_default(&default_dir)
        .prompt()?;
    let dir = expand_home(answer.trim());
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {}", dir.display(), e))?;
    store.config.download_dir = Some(dir.display().to_string());
    store.config.setup_completed = Some(true);
    store.save_config()?;

    println!(
        "\n{} All set! Try `{} courses`, `{} slides <course>`, or just run `{}`.",
        "✓".green().bold(),
        crate::ui::BIN,
        crate::ui::BIN,
        crate::ui::BIN
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_portal_urls() {
        assert_eq!(
            normalize_base_url("lms.example.edu").unwrap(),
            "https://lms.example.edu"
        );
        assert_eq!(
            normalize_base_url(" https://lms.example.edu/ ").unwrap(),
            "https://lms.example.edu"
        );
        assert_eq!(
            normalize_base_url("http://localhost:8080//").unwrap(),
            "http://localhost:8080"
        );
    }

    #[test]
    fn rejects_unusable_portal_urls() {
        assert!(normalize_base_url("   ").is_err());
        assert!(normalize_base_url("ftp://lms.example.edu").is_err());
    }

    #[test]
    fn flags_plain_http_except_loopback() {
        assert!(is_insecure_remote("http://lms.example.edu"));
        assert!(!is_insecure_remote("https://lms.example.edu"));
        assert!(!is_insecure_remote("http://localhost:3000"));
        assert!(!is_insecure_remote("http://127.0.0.1:3000"));
    }

    #[test]
    fn expands_home_prefix() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_home("~/dl"), home.join("dl"));
            assert_eq!(expand_home("~"), home);
        }
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
    }
}
