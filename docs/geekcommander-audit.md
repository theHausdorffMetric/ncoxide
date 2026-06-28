# Security Audit: geekcommander v1.0.5

**Date:** 2026-02-09
**Crate:** geekcommander v1.0.5
**Repository:** https://github.com/akram0zaki/geekcommander
**Author:** Akram Zaki
**Auditor:** Claude Opus 4.6 (automated analysis)

---

## Backdoors / Malicious Code: None Found

The codebase shows no indicators of malicious intent:

- **No network activity** -- no use of `std::net`, no HTTP clients, no sockets, no DNS lookups. The application is entirely offline.
- **No data exfiltration** -- no telemetry, no analytics, no hidden file reads sent elsewhere.
- **No obfuscated code** -- all logic is straightforward and readable.
- **No suspicious process spawning** -- the only `Command::new` call is in `viewer.rs:194` for launching the user's `$EDITOR`, which is legitimate.
- **No build.rs** -- no compile-time code injection.
- **No proc macros** -- only standard `derive` from well-known crates (serde, thiserror, clap).
- **All dependencies are mainstream** -- ratatui, crossterm, chrono, zip, tar, walkdir, clap, serde, toml, etc. No unknown or suspicious crates.

---

## Vulnerabilities

### 1. HIGH: ZIP Slip / Path Traversal in Archive Extraction

**Files:** `src/archive.rs:108-123`, `src/archive.rs:209-229`

Both `ZipHandler::extract_to_disk` and `TarHandler::extract_to_disk` write files to `output_path` without validating that the resolved path stays within the intended extraction directory. A maliciously crafted archive with entries like `../../../etc/crontab` could write files anywhere the user has write access.

```rust
// archive.rs:119 -- no path canonicalization or containment check
let mut output_file = std::fs::File::create(output_path)?;
```

This is a well-documented vulnerability class known as ZIP Slip.

**Recommendation:** Canonicalize the output path and verify it starts with the intended extraction directory before writing.

---

### 2. MEDIUM: Silent File Overwrite -- Unimplemented Safety Check

**Files:** `src/ui.rs:643-645`, `src/core.rs:388`

The config has `confirm_overwrite: true`, but the `ConfirmAction::Overwrite` handler is empty:

```rust
ConfirmAction::Overwrite => {
    // Handle file overwrite confirmation
},
```

Meanwhile, `copy_file_with_progress` at `core.rs:388` uses `fs::File::create(dest)` which silently overwrites existing files. Users get no warning when overwriting files during copy/move operations.

**Recommendation:** Check for destination file existence before copying and prompt the user via the existing `ConfirmAction::Overwrite` dialog path.

---

### 3. MEDIUM: Symlink-Related Risks

**Files:** `src/core.rs:409-429`, `src/core.rs:347`, `src/core.rs:370`, `src/config.rs:64`

- `copy_directory_recursive` follows symlinks without checking, so a symlink pointing outside the source tree will be copied, potentially leaking file content from elsewhere on the system.
- `fs::remove_dir_all` on a path containing symlinks could delete files outside the intended tree in certain race conditions.
- The `follow_symlinks` config option is defined in `config.rs:64` but is never actually consulted anywhere in the codebase -- it is dead configuration.

**Recommendation:** Check whether entries are symlinks during recursive operations and respect the `follow_symlinks` configuration value.

---

### 4. MEDIUM: No Input Sanitization on File/Directory Names

**Files:** `src/core.rs:495-517`

`create_directory` and `rename_file` accept raw user input without sanitizing:

- Path separator characters (`/`, `\`) embedded in names
- Windows reserved names (`CON`, `NUL`, `PRN`, etc.)
- Leading/trailing dots or spaces
- Extremely long names

A user could inadvertently create files that are difficult to manage or that cause unexpected path resolution.

**Recommendation:** Validate and sanitize user-provided file and directory names before passing them to filesystem operations.

---

### 5. LOW: TOCTOU Race Conditions

**Files:** `src/core.rs:498-503`, `src/core.rs:510-517`

Both `create_directory` and `rename_file` check existence before the operation:

```rust
if new_dir.exists() {           // check
    return Err(...);
}
fs::create_dir(&new_dir)?;      // use -- race window between check and use
```

In a single-user TUI app this is low-severity, but it is a correctness issue.

**Recommendation:** Remove the pre-check and handle the error from the filesystem operation directly, or use platform-specific atomic operations.

---

### 6. LOW: Potential Symlink Loop DoS

**Files:** `src/core.rs:439-452`

`get_path_size` recursively traverses directories without cycle detection. A symlink loop could cause unbounded recursion (though OS-level symlink depth limits provide some mitigation). This is called by `calculate_total_size` before every file operation.

**Recommendation:** Track visited inodes/paths or use `walkdir` with its built-in cycle detection instead of manual recursion.

---

### 7. LOW: Memory Consumption in File Viewer

**Files:** `src/viewer.rs:43-44`

`FileViewer::new` reads the entire file into memory via `read_to_end`. The 50MB cap at `viewer.rs:35` limits this, but it still allows a single operation to consume approximately 50MB of heap. No streaming or paged reading is implemented.

**Recommendation:** Consider a streaming or memory-mapped approach for large files.

---

### 8. INFO: build.sh Pipes curl to Shell

**File:** `build.sh:43`

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

This is the standard rustup installation pattern and uses HTTPS with TLS 1.2 enforcement, so it is acceptable. Still worth noting as a pattern that bypasses signature verification.

---

### 9. INFO: build.ps1 Writes to System32

**File:** `build.ps1:121`

The Windows build script falls back to writing `geekcommander.exe` into `C:\Windows\System32`. This is unexpected behavior for a file manager build script and could trigger AV/EDR alerts.

**Recommendation:** Remove the System32 fallback and only install to the user's cargo bin directory.

---

### 10. INFO: Non-functional Disk Space on Linux

**File:** `src/platform.rs:37-40`

```rust
#[cfg(not(windows))]
{
    Ok(1024 * 1024 * 1024) // Return 1GB as fallback
}
```

`get_free_disk_space` returns a hardcoded 1GB on non-Windows platforms. The status bar displays this misleading value to users.

**Recommendation:** Use `libc::statvfs` or the `sys-info` crate to report actual disk space on Unix systems.

---

## Summary

| Severity | Count | Key Issues |
|----------|-------|------------|
| High     | 1     | ZIP Slip path traversal in archive extraction |
| Medium   | 3     | Silent overwrite, symlink following, no input sanitization |
| Low      | 3     | TOCTOU races, symlink loops, memory consumption |
| Info     | 3     | curl pipe to shell, System32 write, hardcoded disk space |

**Overall assessment:** This is a straightforward, non-malicious file manager with no backdoors. The most serious vulnerability is the ZIP Slip path traversal in archive extraction, which could allow a crafted archive to write files to arbitrary locations. The other issues are typical of early-stage file manager implementations that have not undergone security hardening.
