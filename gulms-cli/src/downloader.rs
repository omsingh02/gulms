use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::categorizer::sanitize_filename;
use crate::models::{Config, Material};

pub fn attach_token(url: &str, token: &str) -> String {
    if url.contains("token=") {
        return url.to_string();
    }
    let sep = if url.contains('?') { "&" } else { "?" };
    format!("{}{}{}={}", url, sep, "token", token)
}

pub fn download_material(
    material: &Material,
    config: &Config,
    dest_override: Option<&Path>,
    show_progress: bool,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let fileurl = material
        .fileurl
        .as_deref()
        .ok_or("No file URL available for this material")?;

    let token = config
        .token
        .as_deref()
        .ok_or("No Moodle auth token configured in ~/.config/gulms/config.json")?;

    let target_url = attach_token(fileurl, token);

    // Destination directory
    let base_dest = if let Some(d) = dest_override {
        d.to_path_buf()
    } else {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/faulter"));
        let default_download = config
            .download_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Downloads").join("gulms"));

        let course_folder = if !material.course_acronym.is_empty() {
            &material.course_acronym
        } else {
            &material.course_name
        };

        default_download
            .join(sanitize_filename(course_folder))
            .join(sanitize_filename(material.category.name()))
    };

    fs::create_dir_all(&base_dest)?;

    let clean_fname = sanitize_filename(&material.filename);
    let target_path = base_dest.join(&clean_fname);

    // Check if exists with matching size
    if target_path.exists() {
        if let Ok(meta) = fs::metadata(&target_path) {
            if material.filesize > 0 && meta.len() == material.filesize {
                if show_progress {
                    println!(
                        "{} {} {}",
                        "•".dimmed(),
                        "Already downloaded:".dimmed(),
                        target_path.display().to_string().cyan()
                    );
                }
                return Ok(target_path);
            }
        }
    }

    if show_progress {
        println!(
            "{} Downloading {} ({})",
            "⬇".cyan().bold(),
            clean_fname.bold(),
            material.size_human().dimmed()
        );
    }

    let mut response = ureq::get(&target_url)
        .header("User-Agent", "Mozilla/5.0 (MoodleMobile; Android)")
        .call()?;

    let total_bytes = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(material.filesize);

    let pb = if show_progress {
        let p = if total_bytes > 0 {
            ProgressBar::new(total_bytes)
        } else {
            ProgressBar::new_spinner()
        };
        p.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("━╸ "),
        );
        Some(p)
    } else {
        None
    };

    let temp_path = base_dest.join(format!(".{}.part", clean_fname));
    let mut file = File::create(&temp_path)?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0u8; 65536];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])?;
        if let Some(ref p) = pb {
            p.inc(bytes_read as u64);
        }
    }

    file.flush()?;
    drop(file);

    fs::rename(&temp_path, &target_path)?;

    if let Some(p) = pb {
        p.finish_and_clear();
        println!(
            "{} Saved to: {}",
            "✓".green().bold(),
            target_path.display().to_string().bold()
        );
    }

    Ok(target_path)
}

pub fn open_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();

    if ext == "pdf" {
        // Try zathura first
        if Command::new("which")
            .arg("zathura")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            Command::new("zathura")
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            return Ok(());
        }
    }

    // Default to xdg-open
    Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
}

pub fn copy_to_clipboard(text: &str) -> bool {
    // Try wl-copy first
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
    }

    // Try xclip
    if let Ok(mut child) = Command::new("xclip")
        .arg("-selection")
        .arg("clipboard")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
    }

    false
}
