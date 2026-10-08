use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::categorizer::sanitize_filename;
use crate::http;
use crate::models::{Config, Material};
use crate::sync::NOT_SIGNED_IN;

pub fn attach_token(url: &str, token: &str) -> String {
    if url.contains("token=") {
        return url.to_string();
    }
    let sep = if url.contains('?') { "&" } else { "?" };
    format!("{}{}{}={}", url, sep, "token", token)
}

/// Download a material into memory (used for slide analysis and note export).
pub fn fetch_bytes(
    material: &Material,
    config: &Config,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let fileurl = material
        .fileurl
        .as_deref()
        .ok_or("No file URL available for this material")?;
    let token = config.token.as_deref().ok_or(NOT_SIGNED_IN)?;

    let mut response =
        http::with_retries(|| http::agent().get(&attach_token(fileurl, token)).call())?;
    let mut bytes = Vec::with_capacity(material.filesize as usize);
    response.body_mut().as_reader().read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Download to disk. `file_name` replaces the stored name (e.g. a canonical lecture name).
pub fn download_material(
    material: &Material,
    config: &Config,
    dest_override: Option<&Path>,
    show_progress: bool,
    file_name: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let fileurl = material
        .fileurl
        .as_deref()
        .ok_or("No file URL available for this material")?;

    let token = config.token.as_deref().ok_or(NOT_SIGNED_IN)?;

    let target_url = attach_token(fileurl, token);

    let base_dest = match dest_override {
        Some(d) => d.to_path_buf(),
        None => material_dir(material, config),
    };

    fs::create_dir_all(&base_dest)?;

    let clean_fname = sanitize_filename(file_name.unwrap_or(&material.filename));
    let target_path = base_dest.join(&clean_fname);

    // Skip the download if an identical-size copy is already on disk
    if material.filesize > 0
        && fs::metadata(&target_path).is_ok_and(|meta| meta.len() == material.filesize)
    {
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

    if show_progress {
        println!(
            "{} Downloading {} ({})",
            "⬇".cyan().bold(),
            clean_fname.bold(),
            material.size_human().dimmed()
        );
    }

    let mut response = http::with_retries(|| http::agent().get(&target_url).call())?;

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
    let copied = (|| -> std::io::Result<()> {
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
        file.flush()
    })();

    // Never leave a truncated `.part` file behind on a failed transfer
    if let Err(e) = copied {
        let _ = fs::remove_file(&temp_path);
        if let Some(p) = pb {
            p.abandon();
        }
        return Err(e.into());
    }

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

/// Where a material is stored: `<download_dir>/<course>/<category>`.
pub fn material_dir(material: &Material, config: &Config) -> PathBuf {
    let course_folder = if material.course_acronym.is_empty() {
        &material.course_name
    } else {
        &material.course_acronym
    };

    config
        .download_dir()
        .join(sanitize_filename(course_folder))
        .join(sanitize_filename(material.category.name()))
}
