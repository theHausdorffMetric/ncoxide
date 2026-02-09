use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::SystemTime;

use jiff::Timestamp;

/// Format file size for display: B, KB, MB, GB, TB.
pub fn format_file_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if size < KB {
        format!("{size} B")
    } else if size < MB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else if size < GB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size < TB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else {
        format!("{:.1} TB", size as f64 / TB as f64)
    }
}

/// Format SystemTime as "YYYY-MM-DD HH:MM" using jiff.
pub fn format_file_time(time: SystemTime) -> String {
    let ts = Timestamp::try_from(time);
    let Ok(ts) = ts else {
        return "???".to_string();
    };
    let zdt = ts.to_zoned(jiff::tz::TimeZone::system());
    zdt.strftime("%Y-%m-%d %H:%M").to_string()
}

/// Unix permission string: "drwxrwxrwx" or "-rwxrwxrwx".
pub fn format_permissions(metadata: &std::fs::Metadata) -> String {
    let mode = metadata.permissions().mode();
    let file_type = if metadata.is_dir() {
        'd'
    } else if metadata.is_symlink() {
        'l'
    } else {
        '-'
    };

    let mut s = String::with_capacity(10);
    s.push(file_type);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Get default editor from $EDITOR, fallback to "vi".
pub fn get_default_editor() -> String {
    std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string())
}

/// Get free disk space for a given path (Linux statvfs).
pub fn get_free_disk_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();

    // SAFETY: We pass a valid null-terminated path and a valid pointer.
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };

    if result == 0 {
        let stat = unsafe { stat.assume_init() };
        Some(stat.f_bavail * stat.f_frsize)
    } else {
        None
    }
}

/// Shorten a path for display: replace home dir with ~.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_size() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(1048576), "1.0 MB");
        assert_eq!(format_file_size(1073741824), "1.0 GB");
    }

    #[test]
    fn test_format_permissions() {
        // We can't easily test this without real metadata, but at least check the function exists
        let meta = std::fs::metadata("/").unwrap();
        let perms = format_permissions(&meta);
        assert_eq!(perms.len(), 10);
        assert!(perms.starts_with('d'));
    }

    #[test]
    fn test_display_path() {
        let home = dirs::home_dir().unwrap();
        let test_path = home.join("foo/bar");
        assert_eq!(display_path(&test_path), "~/foo/bar");
        assert_eq!(display_path(Path::new("/etc/hosts")), "/etc/hosts");
    }
}
