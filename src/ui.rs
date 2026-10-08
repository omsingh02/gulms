use colored::*;

use crate::store::LmsStore;

/// Binary name, used in hints so they stay correct if the crate is renamed.
pub const BIN: &str = env!("CARGO_PKG_NAME");

/// Truncate to at most `max_len` characters, marking the cut with `...`.
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    let keep = max_len.saturating_sub(3);
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}

/// A `[ACRONYM]` badge padded to `width` *before* colouring, so ANSI escapes
/// don't throw off column alignment.
pub fn badge(acronym: &str, width: usize) -> ColoredString {
    format!("{:<width$}", format!("[{}]", acronym))
        .bold()
        .cyan()
}

/// Width needed to align every `[ACRONYM]` badge in a list.
pub fn badge_width<'a>(acronyms: impl Iterator<Item = &'a str>) -> usize {
    acronyms.map(|a| a.chars().count() + 2).max().unwrap_or(0)
}

pub fn print_account_info(store: &LmsStore) {
    let cfg = &store.config;
    let row = |label: &str, value: String| println!("  {:<20} {}", label.bold(), value);

    println!("\n{}", "═══ GULMS Account & System Info ═══".bold().cyan());
    row("Base Portal:", cfg.base_url.clone());
    row(
        "Student Name:",
        cfg.fullname.as_deref().unwrap_or("Not set").to_string(),
    );
    row(
        "Username:",
        cfg.username.as_deref().unwrap_or("Not set").to_string(),
    );
    row(
        "User ID:",
        cfg.user_id
            .map_or_else(|| "Not set".to_string(), |u| u.to_string()),
    );
    row(
        "Token Configured:",
        if cfg.token.is_some() {
            "Yes".green().to_string()
        } else {
            format!("No (add \"token\" to {})", store.config_path.display())
                .red()
                .to_string()
        },
    );
    row(
        "Download Directory:",
        store
            .download_dir()
            .display()
            .to_string()
            .cyan()
            .to_string(),
    );
    row("Config File:", store.config_path.display().to_string());
    row(
        "Courses Cache:",
        store.courses_cache_path.display().to_string(),
    );
    row("PPT Metadata:", store.ppt_meta_path.display().to_string());
    row(
        "Cached Courses:",
        format!("{} courses", store.courses.len()),
    );
    row(
        "Last Sync:",
        store
            .sync_state
            .as_ref()
            .and_then(|s| s.last_sync_human.as_deref())
            .unwrap_or("Never")
            .yellow()
            .to_string(),
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_marks_cut_and_respects_limit() {
        assert_eq!(truncate("abcdefgh", 6), "abc...");
        assert_eq!(truncate("abcdefgh", 6).chars().count(), 6);
    }

    #[test]
    fn truncate_handles_tiny_limits_and_unicode() {
        assert_eq!(truncate("abcdef", 2), "...");
        assert_eq!(truncate("ééééé", 4), "é...");
    }

    #[test]
    fn badge_width_covers_brackets() {
        assert_eq!(badge_width(["DBMS", "COA"].into_iter()), 6);
        assert_eq!(badge_width(std::iter::empty()), 0);
    }
}
