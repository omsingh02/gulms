//! Thin OS-specific helpers: opening files/folders and the clipboard.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::models::Config;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Program that opens a file or folder with the user's default application.
fn system_opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    }
}

fn spawn_detached(program: &str, arg: &Path) -> std::io::Result<()> {
    Command::new(program)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Open a downloaded file. PDFs go to `viewer` from the config if set; on Linux
/// zathura is used when installed. Everything else uses the system default.
pub fn open_file(path: &Path, config: &Config) -> Result<()> {
    let is_pdf = path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));

    if is_pdf {
        if let Some(viewer) = config.viewer.as_deref().filter(|v| !v.trim().is_empty()) {
            return spawn_detached(viewer, path).map_err(|e| {
                format!("could not launch configured viewer '{}': {}", viewer, e).into()
            });
        }
        if cfg!(target_os = "linux") && spawn_detached("zathura", path).is_ok() {
            return Ok(());
        }
    }

    open_with_system(path)
}

/// Open a folder in the system file manager.
pub fn open_folder(path: &Path) -> Result<()> {
    open_with_system(path)
}

fn open_with_system(path: &Path) -> Result<()> {
    let opener = system_opener();
    spawn_detached(opener, path).map_err(|e| {
        format!(
            "could not launch {} ({}); open {} manually",
            opener,
            e,
            path.display()
        )
        .into()
    })
}

/// Candidate clipboard programs for this OS, tried in order.
fn clipboard_commands() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(windows) {
        &[("clip", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

/// Copy text to the clipboard using whichever tool is available. Returns false
/// when none worked, so the caller can print the text instead.
pub fn copy_to_clipboard(text: &str) -> bool {
    for (program, args) in clipboard_commands() {
        let Ok(mut child) = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }
    false
}
