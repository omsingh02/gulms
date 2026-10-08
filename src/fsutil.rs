use std::fs;
use std::io::Write;
use std::path::Path;

/// Write `contents` to `path` atomically (temp file + rename), so a crash or
/// Ctrl-C mid-write can never leave a truncated file behind. With `private`,
/// the file is created readable by the owner only (0600 on Unix), which is what
/// you want for anything holding a credential.
pub fn atomic_write(path: &Path, contents: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);

    let result = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    // `private` only matters on Unix; Windows uses per-user profile ACLs.
    let _ = private;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gulms-fsutil-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_and_replaces_without_leaving_temp_files() {
        let dir = scratch("replace");
        let path = dir.join("nested").join("file.json");

        atomic_write(&path, b"one", false).unwrap();
        atomic_write(&path, b"two", false).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temp file should be renamed away");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        let path = dir.join("config.json");

        atomic_write(&path, b"secret", true).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
