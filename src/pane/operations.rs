use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NcError, Result};
use crate::platform;

/// Refuse a transfer whose destination entry is the source itself or lies
/// inside a source directory (`cp`/`mv`-style guard). Both are destructive:
/// `fs::copy` onto the same path truncates the file to zero bytes before
/// reading, and copying a directory into its own subtree recurses into the
/// tree it is creating.
fn ensure_safe_transfer(source: &Path, target_dir: &Path) -> Result<()> {
    let name = source
        .file_name()
        .ok_or_else(|| NcError::FileOperation("Invalid source path".into()))?;
    // Canonicalize the source's *parent* and re-attach the final component,
    // so a symlink source compares as the link entry itself, not its target.
    let src_parent = match source.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let src = fs::canonicalize(src_parent)
        .map_err(NcError::Io)?
        .join(name);
    let dest = fs::canonicalize(target_dir)
        .map_err(NcError::Io)?
        .join(name);

    if dest == src {
        return Err(NcError::FileOperation(format!(
            "source and destination are the same: {}",
            src.display()
        )));
    }
    if dest.starts_with(&src) {
        return Err(NcError::FileOperation(format!(
            "cannot transfer '{}' into itself ('{}')",
            source.display(),
            dest.display()
        )));
    }
    Ok(())
}

/// Copy a file or directory to a target directory.
/// Symlinks are preserved as symlinks rather than followed.
/// Refuses self- and nested-destination transfers (see [`ensure_safe_transfer`]).
pub fn copy_to(source: &Path, target_dir: &Path) -> Result<()> {
    ensure_safe_transfer(source, target_dir)?;
    let name = source
        .file_name()
        .ok_or_else(|| NcError::FileOperation("Invalid source path".into()))?;
    let dest = target_dir.join(name);

    let meta = fs::symlink_metadata(source).map_err(NcError::Io)?;
    if meta.is_symlink() {
        let link_target = fs::read_link(source).map_err(NcError::Io)?;
        // Remove existing destination if present so symlink creation succeeds
        if dest.symlink_metadata().is_ok() {
            fs::remove_file(&dest).map_err(NcError::Io)?;
        }
        std::os::unix::fs::symlink(&link_target, &dest).map_err(NcError::Io)?;
        Ok(())
    } else if meta.is_dir() {
        copy_dir_recursive(source, &dest)
    } else {
        fs::copy(source, &dest).map_err(NcError::Io)?;
        Ok(())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(NcError::Io)?;
    for entry in fs::read_dir(src).map_err(NcError::Io)? {
        let entry = entry.map_err(NcError::Io)?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&src_path).map_err(NcError::Io)?;
        if meta.is_symlink() {
            let link_target = fs::read_link(&src_path).map_err(NcError::Io)?;
            if dst_path.symlink_metadata().is_ok() {
                fs::remove_file(&dst_path).map_err(NcError::Io)?;
            }
            std::os::unix::fs::symlink(&link_target, &dst_path).map_err(NcError::Io)?;
        } else if meta.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(NcError::Io)?;
        }
    }
    Ok(())
}

/// Calculate total size of a path (recursive for directories).
/// Uses symlink_metadata to avoid following symlinks.
pub fn path_size(path: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(path).map_err(NcError::Io)?;
    if meta.is_symlink() || meta.is_file() {
        Ok(meta.len())
    } else if meta.is_dir() {
        let mut total = 0u64;
        for entry in fs::read_dir(path).map_err(NcError::Io)? {
            let entry = entry.map_err(NcError::Io)?;
            // Propagate errors so an unreadable subtree doesn't silently
            // under-count the size used by the disk-space pre-check.
            total = total.saturating_add(path_size(&entry.path())?);
        }
        Ok(total)
    } else {
        Ok(0)
    }
}

/// Move a file or directory to a target directory.
/// Refuses self- and nested-destination transfers (see [`ensure_safe_transfer`]).
pub fn move_to(source: &Path, target_dir: &Path) -> Result<()> {
    ensure_safe_transfer(source, target_dir)?;
    let name = source
        .file_name()
        .ok_or_else(|| NcError::FileOperation("Invalid source path".into()))?;
    let dest = target_dir.join(name);

    // Try rename first (same filesystem), fall back to copy+delete
    match fs::rename(source, &dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Unlike the rename above, copy+delete needs free space at the
            // destination — fail before copying anything partial. Fail-open
            // when free space or source size can't be determined.
            check_fallback_space(source, target_dir)?;
            copy_to(source, target_dir)?;
            if let Err(e) = delete(source) {
                return Err(NcError::FileOperation(format!(
                    "Copied to {} but failed to remove source: {e}",
                    dest.display()
                )));
            }
            Ok(())
        }
    }
}

/// Space check for `move_to`'s copy+delete fallback. Errors only when both
/// the destination's free space and the source size are known and the source
/// doesn't fit (see [`space_shortfall`]).
fn check_fallback_space(source: &Path, target_dir: &Path) -> Result<()> {
    let Some(free) = platform::get_free_disk_space(target_dir) else {
        return Ok(());
    };
    let Ok(needed) = path_size(source) else {
        return Ok(());
    };
    match space_shortfall(needed, free) {
        Some(msg) => Err(NcError::FileOperation(msg)),
        None => Ok(()),
    }
}

/// `Some(message)` when `needed` bytes don't fit into `free` bytes.
fn space_shortfall(needed: u64, free: u64) -> Option<String> {
    (needed > free).then(|| {
        format!(
            "not enough disk space: need {} but only {} available",
            platform::format_file_size(needed),
            platform::format_file_size(free),
        )
    })
}

/// Delete a file or directory (recursive for directories).
/// Uses symlink_metadata so symlinks are removed as links, not followed.
pub fn delete(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(NcError::Io)?;
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(NcError::Io)
    } else {
        // Covers regular files, symlinks, and other non-directory types
        fs::remove_file(path).map_err(NcError::Io)
    }
}

/// Rename a file or directory.
pub fn rename(source: &Path, new_name: &str) -> Result<PathBuf> {
    let parent = source
        .parent()
        .ok_or_else(|| NcError::FileOperation("Cannot rename root".into()))?;
    let dest = parent.join(new_name);
    fs::rename(source, &dest).map_err(NcError::Io)?;
    Ok(dest)
}

/// Create a new directory.
///
/// Uses `create_dir` (not `create_dir_all`) so it creates exactly one
/// directory and errors if the target already exists, preserving the
/// caller's collision handling.
pub fn mkdir(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    fs::create_dir(&path).map_err(NcError::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_copy_file() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_copy_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("file.txt"), "hello").unwrap();

        copy_to(&src.join("file.txt"), &dst).unwrap();
        assert!(dst.join("file.txt").exists());
        assert_eq!(fs::read_to_string(dst.join("file.txt")).unwrap(), "hello");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_copy_dir() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_copydir_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        let subdir = src.join("inner");
        fs::create_dir_all(&subdir).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(subdir.join("file.txt"), "nested").unwrap();

        copy_to(&src, &dst).unwrap();
        assert!(dst.join("src").join("inner").join("file.txt").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_move_file() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_move_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src_dir = tmp.join("src");
        let dst_dir = tmp.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        fs::write(src_dir.join("file.txt"), "moveme").unwrap();

        move_to(&src_dir.join("file.txt"), &dst_dir).unwrap();
        assert!(!src_dir.join("file.txt").exists());
        assert!(dst_dir.join("file.txt").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_delete() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_delete_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("file.txt"), "deleteme").unwrap();

        delete(&tmp.join("file.txt")).unwrap();
        assert!(!tmp.join("file.txt").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rename() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_rename_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("old.txt"), "renamed").unwrap();

        let new_path = rename(&tmp.join("old.txt"), "new.txt").unwrap();
        assert!(!tmp.join("old.txt").exists());
        assert_eq!(new_path, tmp.join("new.txt"));
        assert!(new_path.exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_mkdir() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_mkdir_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let new_dir = mkdir(&tmp, "newdir").unwrap();
        assert!(new_dir.is_dir());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_mkdir_existing_errors() {
        let tmp = std::env::temp_dir().join(format!(
            "ncoxide_test_mkdir_existing_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // First creation succeeds; a second with the same name must error
        // (create_dir, not create_dir_all) so collision handling is preserved.
        mkdir(&tmp, "dup").unwrap();
        assert!(mkdir(&tmp, "dup").is_err());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_space_shortfall_comparison() {
        // Locks the comparison direction for the fallback space check (R5):
        // only a genuine shortfall errors; exact fit passes.
        assert!(space_shortfall(10, 5).is_some());
        assert!(space_shortfall(5, 10).is_none());
        assert!(space_shortfall(5, 5).is_none());
    }

    /// Unique per-process temp dir for the transfer-guard tests (R1).
    fn guard_dir(name: &str) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(format!("ncoxide_guard_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn test_copy_file_onto_itself_refused() {
        // fs::copy(src, src) truncates the file to zero bytes — must refuse.
        let tmp = guard_dir("self_file");
        fs::write(tmp.join("data.txt"), "important").unwrap();

        assert!(copy_to(&tmp.join("data.txt"), &tmp).is_err());
        assert_eq!(
            fs::read_to_string(tmp.join("data.txt")).unwrap(),
            "important"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_copy_dir_onto_itself_refused() {
        let tmp = guard_dir("self_dir");
        let sub = tmp.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("f.txt"), "keep").unwrap();

        assert!(copy_to(&sub, &tmp).is_err());
        assert_eq!(fs::read_to_string(sub.join("f.txt")).unwrap(), "keep");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_copy_dir_into_own_subdir_refused() {
        // Copying /x into /x/inner would recurse into the tree being created.
        let tmp = guard_dir("nested");
        let src = tmp.join("src_dir");
        let inner = src.join("inner");
        fs::create_dir_all(&inner).unwrap();

        assert!(copy_to(&src, &inner).is_err());
        assert!(!inner.join("src_dir").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_copy_into_symlinked_same_dir_refused() {
        // The target dir is a symlink resolving to the source's own directory;
        // the guard must see through it.
        let tmp = guard_dir("symlink_target");
        fs::write(tmp.join("data.txt"), "important").unwrap();
        let alias =
            std::env::temp_dir().join(format!("ncoxide_guard_alias_{}", std::process::id()));
        let _ = fs::remove_file(&alias);
        std::os::unix::fs::symlink(&tmp, &alias).unwrap();

        assert!(copy_to(&tmp.join("data.txt"), &alias).is_err());
        assert_eq!(
            fs::read_to_string(tmp.join("data.txt")).unwrap(),
            "important"
        );

        let _ = fs::remove_file(&alias);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_move_file_onto_itself_refused() {
        let tmp = guard_dir("move_self");
        fs::write(tmp.join("data.txt"), "important").unwrap();

        assert!(move_to(&tmp.join("data.txt"), &tmp).is_err());
        assert_eq!(
            fs::read_to_string(tmp.join("data.txt")).unwrap(),
            "important"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_move_dir_into_own_subdir_refused() {
        let tmp = guard_dir("move_nested");
        let src = tmp.join("src_dir");
        let inner = src.join("inner");
        fs::create_dir_all(&inner).unwrap();
        fs::write(src.join("f.txt"), "keep").unwrap();

        assert!(move_to(&src, &inner).is_err());
        assert!(src.is_dir());
        assert_eq!(fs::read_to_string(src.join("f.txt")).unwrap(), "keep");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_path_size_recursive() {
        let tmp =
            std::env::temp_dir().join(format!("ncoxide_test_path_size_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let inner = tmp.join("inner");
        fs::create_dir_all(&inner).unwrap();
        fs::write(tmp.join("a.txt"), "12345").unwrap(); // 5 bytes
        fs::write(inner.join("b.txt"), "678").unwrap(); // 3 bytes

        let size = path_size(&tmp).unwrap();
        assert_eq!(size, 8, "expected sum of nested file sizes");

        let _ = fs::remove_dir_all(&tmp);
    }
}
