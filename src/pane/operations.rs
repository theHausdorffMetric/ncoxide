use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NcError, Result};

/// Copy a file or directory to a target directory.
/// Symlinks are preserved as symlinks rather than followed.
pub fn copy_to(source: &Path, target_dir: &Path) -> Result<()> {
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
            total += path_size(&entry.path()).unwrap_or(0);
        }
        Ok(total)
    } else {
        Ok(0)
    }
}

/// Move a file or directory to a target directory.
pub fn move_to(source: &Path, target_dir: &Path) -> Result<()> {
    let name = source
        .file_name()
        .ok_or_else(|| NcError::FileOperation("Invalid source path".into()))?;
    let dest = target_dir.join(name);

    // Try rename first (same filesystem), fall back to copy+delete
    match fs::rename(source, &dest) {
        Ok(()) => Ok(()),
        Err(_) => {
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
pub fn mkdir(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    fs::create_dir_all(&path).map_err(NcError::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_copy_file() {
        let tmp = std::env::temp_dir().join("ncoxide_test_copy");
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
        let tmp = std::env::temp_dir().join("ncoxide_test_copydir");
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
        let tmp = std::env::temp_dir().join("ncoxide_test_move");
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
        let tmp = std::env::temp_dir().join("ncoxide_test_delete");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("file.txt"), "deleteme").unwrap();

        delete(&tmp.join("file.txt")).unwrap();
        assert!(!tmp.join("file.txt").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rename() {
        let tmp = std::env::temp_dir().join("ncoxide_test_rename");
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
        let tmp = std::env::temp_dir().join("ncoxide_test_mkdir");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let new_dir = mkdir(&tmp, "newdir").unwrap();
        assert!(new_dir.is_dir());

        let _ = fs::remove_dir_all(&tmp);
    }
}
