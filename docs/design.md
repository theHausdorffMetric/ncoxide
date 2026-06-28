# ncoxide — Modal Dual-Pane File Commander

## Table of Contents
1. [GeekCommander Analysis](#1-geekcommander-analysis)
2. [ncoxide Design & Plan](#2-ncoxide-design--plan)
3. [Implementation Status](#3-implementation-status)

---

## 1. GeekCommander Analysis

### Overview

geekcommander is a Norton Commander-style dual-pane file manager written in Rust.
3,626 lines across 8 source files, 40 tests, using deprecated `tui 0.19`.

### Source Structure

| File | Lines | Purpose |
|------|------:|---------|
| `core.rs` | 1,110 | FileEntry, PaneState, file ops (copy/move/delete), archive context, glob |
| `ui.rs` | 896 | App struct, event loop (50ms poll), dual-pane rendering, dialog system |
| `config.rs` | 569 | INI/TOML config, keybindings (F-keys), color scheme, panel paths |
| `viewer.rs` | 347 | File viewer (50MB limit, binary detection, external $EDITOR) |
| `archive.rs` | 330 | ZIP/TAR handler trait + impls, archive navigation |
| `platform.rs` | 290 | Cross-platform (Windows winapi, disk space, permissions, paths) |
| `error.rs` | 45 | GeekCommanderError enum (thiserror 1.x) |
| `main.rs` | 39 | Logger init, config load, App::new().run() |
| **Total** | **3,626** | |

### Dependencies

```
tui 0.19           — deprecated since 2023 (replaced by ratatui)
crossterm 0.27     — terminal backend
thiserror 1.0      — error handling
chrono 0.4         — date/time
zip 0.6, tar 0.4   — archive support
clap 4.0           — CLI args
serde 1.0, toml 0.8, ini 1.3  — config
walkdir 2.3, dirs 5.0, log 0.4, fern 0.6, env_logger 0.10
winapi 0.3         — Windows support
```

### Architecture Assessment

**Strengths:**
- Clean PaneState struct with cursor, selections, directory listing
- Separation of core logic from UI rendering
- Dialog system (Confirm, Input, Error, Progress, Help)
- Comprehensive test coverage (40 tests: 22 core, 6 viewer, 6 platform, 6 config)
- Cross-platform (Windows via winapi)
- Archive browsing without extraction

**Weaknesses:**
- Uses deprecated `tui 0.19` (should be ratatui 0.30+)
- Clones entire app state every frame
- F-key bindings are a Norton Commander relic — not ergonomic on modern keyboards
- Flat key dispatch (huge if/else chain in `handle_key_event`)
- No hjkl navigation, no modal awareness
- Archive code tangled into core types (`is_archive` field, `ArchiveContext`)
- App owns Terminal directly — hard to test
- INI config format alongside TOML — redundant

### Key Code Worth Porting
- `core.rs:65-248` — PaneState navigation logic (adapted to modal style)
- `core.rs:251-518` — File operations (modernized with better error handling)
- `ui.rs:709-817` — Pane rendering pattern (ported to ratatui 0.30)
- `platform.rs` — Most of this reused directly (minus Windows code)
- `viewer.rs` — Reused with syntect highlighting added

---

## 2. ncoxide Design & Plan

### Core Philosophy

Helix-inspired **selection → action** modal interface. Navigate and see your target, then act. Seven modes with Esc always returning to Normal.

### Mode Transition Diagram

```
                    ┌──────────────────────────┐
                    │      NORMAL MODE         │ ← default, Esc returns here
                    │  navigate, preview       │
                    └──────┬───────────────────┘
                           │
         ┌─────────┬───────┼────────┬──────────┬──────────┬──────────┐
         │         │       │        │          │          │          │
         v         v       v        v          v          v          v
    ┌─────────┐ ┌──────┐ ┌─────┐ ┌──────┐ ┌────────┐ ┌────────┐ ┌────────┐
    │ Select  │ │Space │ │Goto │ │  :   │ │ Input  │ │ Finder │ │ View   │
    │  (v)    │ │(Spc) │ │ (g) │ │Cmd   │ │(rename)│ │(Spc-f) │ │(Enter) │
    │ mark    │ │menu  │ │jump │ │mode  │ │ mkdir  │ │ fuzzy  │ │file    │
    │ files   │ │ops   │ │to   │ │      │ │ search │ │ find   │ │viewer  │
    └─────────┘ └──────┘ └─────┘ └──────┘ └────────┘ └────────┘ └────────┘
         │         │       │        │          │          │          │
         └─────────┴───────┴────────┴──────────┴──────────┴──────────┘
                                    │
                             returns to NORMAL
```

### Keymap

#### Normal Mode (default)
| Key | Action |
|-----|--------|
| `j` / `k` / `Up` / `Down` | Cursor down / up |
| `h` / `Backspace` | Parent directory |
| `l` / `Enter` | Enter directory / open file |
| `J` / `K` | Move cursor + select (extend) |
| `G` | Jump to bottom |
| `gg` | Jump to top |
| `Ctrl-d` / `Ctrl-u` | Half-page down / up |
| `Tab` | Toggle active pane |
| `/` | Search/filter files |
| `v` | Enter Select mode |
| `Space` | Enter Space mode (leader) |
| `g` | Enter Goto mode |
| `:` | Enter Command mode |
| `.` | Toggle hidden files |
| `P` | Toggle Preview mode |
| `r` | Quick rename (Input mode) |
| `d` | Delete (with confirm dialog) |
| `y` | Yank (copy path to register) |
| `p` | Paste (from register to current dir) |
| `?` | Help overlay |
| `q` | Quit |

#### Select Mode (`v`)
| Key | Action |
|-----|--------|
| `j` / `k` | Move + extend selection |
| `v` | Toggle individual |
| `a` | Select all |
| `n` | Invert selection |
| `*` | Select by glob pattern |
| `d` / `y` / `p` | Delete / yank / paste selected |
| `Space` | Enter Space mode with selection |
| `Esc` | Clear selection, return to Normal |

#### Space Mode (leader menu popup)
| Key | Action |
|-----|--------|
| `c` | Copy to other pane |
| `m` | Move to other pane |
| `d` | Delete |
| `r` | Rename |
| `n` | New directory |
| `e` | Edit with $EDITOR |
| `f` | Fuzzy file finder |
| `s` | Sort menu |
| `i` | File info |
| `b` | Bookmarks |
| `?` | Show all commands |

#### Goto Mode (`g`)
| Key | Action |
|-----|--------|
| `g` | Top of list |
| `e` | End of list |
| `h` | Home directory |
| `r` | Root `/` |
| `o` | Other pane's directory |
| `p` | Previous directory (back) |
| `1`-`9` | Bookmark N |

#### Command Mode (`:`)
| Command | Action |
|---------|--------|
| `:q` | Quit |
| `:sort name/size/date/ext` | Change sort |
| `:filter <pattern>` | Filter visible files |
| `:cd <path>` | Change directory |
| `:shell <cmd>` | Run shell command |
| `:set show_hidden` | Toggle setting |

#### Preview Mode (`P` toggle)
- Repurposes inactive pane to show syntax-highlighted file content
- Cursor movement in file pane auto-updates preview
- Focus on preview pane: Up/Down scrolls file content
- `P` again restores inactive pane to directory listing

### Module Structure

```
ncoxide/src/
├── main.rs              — CLI args (clap), logging (fern), App::run()
├── lib.rs               — Module declarations
├── error.rs             — NcError enum (thiserror 2)
├── config.rs            — TOML config (~/.config/ncoxide/config.toml)
├── app.rs               — App struct, event loop, mode dispatch, action handlers
├── finder.rs            — Fuzzy file finder (nucleo-matcher)
├── preview.rs           — Syntax-highlighted preview (syntect)
├── viewer.rs            — Full-screen file viewer
├── platform.rs          — Unix helpers (disk space, permissions, paths)
├── mode/
│   ├── mod.rs           — Mode enum, InputKind enum, Action enum (~50 variants)
│   ├── normal.rs        — Normal mode (multi-key gg support)
│   ├── select.rs        — Select mode
│   ├── space.rs         — Space leader menu
│   ├── goto.rs          — Goto mode
│   ├── command.rs       — Command mode + parser
│   └── input.rs         — Text input (rename, mkdir, search, glob)
├── pane/
│   ├── mod.rs           — FileEntry, PaneState, PaneId, SortBy
│   ├── navigation.rs    — Cursor, enter dir, parent, goto, scroll
│   ├── selection.rs     — Toggle, all, invert, glob, extend
│   └── operations.rs    — Copy, move, delete, rename, mkdir
└── ui/
    ├── mod.rs           — Top-level draw(), space menu overlay, finder overlay
    ├── pane_view.rs     — Dual-pane + preview-pane rendering
    ├── status_line.rs   — Mode indicator, path, selection count
    ├── dialog.rs        — Confirm, error, info dialog overlays
    └── help.rs          — Help overlay with keymap reference
```

### Tech Stack

| Crate | Version | Purpose |
|-------|---------|---------|
| ratatui | 0.30 | Terminal UI framework |
| crossterm | 0.29 | Terminal backend (event-stream) |
| thiserror | 2 | Error derive macros |
| serde | 1 | Config serialization |
| toml | 0.8 | Config file format |
| clap | 4 | CLI argument parsing |
| jiff | 0.2 | Date/time (consistent with qloxide) |
| walkdir | 2 | Recursive directory traversal |
| dirs | 5 | XDG directory paths |
| log + fern | 0.4 / 0.6 | Logging |
| libc | 0.2 | statvfs for disk space |
| unicode-width | 0.2 | Terminal column widths |
| nucleo-matcher | 0.3 | Fuzzy find (helix's engine) |
| syntect | 5 | Syntax highlighting |
| syntect-tui | 3 | syntect → ratatui Span conversion |

### Architectural Decisions

1. **Elm Architecture (TEA):** `update(state, event) → state` + `view(state) → frame`. No per-frame cloning.
2. **Mode dispatch via enum + match:** Each mode returns an `Action` enum variant. App dispatches actions to pane/operations.
3. **No archive support:** Clean break from geekcommander. If added later, separate module behind feature flag.
4. **TOML config** at `~/.config/ncoxide/config.toml` (XDG compliant).
5. **Linux-only:** No Windows/winapi. Direct Unix APIs. Simplifies platform.rs.
6. **Edition 2024:** Rust 2024 edition with its stricter borrowing rules.

---

## 3. Implementation Status

### Build Status

| Metric | Status |
|--------|--------|
| `cargo build` | Clean |
| `cargo clippy` | Zero warnings |
| `cargo test` | **27 passed**, 0 failed |
| Edition | Rust 2024 |
| Git | 5 commits (latest: `cbc1aa8`) |
| Location | `~/dev/nc/ncoxide/` |
| Origin | `git@git.sr.ht:~dpclaude/ncoxide` (dev) |
| Upstream | `git@git.sr.ht:~danprobst/ncoxide` (release) |
| Tracker | `~danprobst/ncoxide-dev` (todo.sr.ht, id: 19112) |

### Code Statistics

| Component | Files | Lines |
|-----------|------:|------:|
| Core (app, config, error, lib, main) | 5 | 895 |
| Modes (normal, select, space, goto, command, input) | 7 | 337 |
| Pane (state, navigation, selection, operations) | 4 | 769 |
| UI (draw, pane_view, status_line, dialog, help) | 5 | 545 |
| Platform + Preview + Viewer + Finder | 4 | 610 |
| **Total** | **25** | **3,131** |

### Test Coverage

| Module | Tests | What's Tested |
|--------|------:|---------------|
| config | 3 | Default config, roundtrip serialize, TOML parsing |
| finder | 2 | Basic fuzzy match, empty query |
| pane/mod | 4 | State creation, sort-by-name, hidden filter, selected paths |
| pane/navigation | 3 | Cursor movement, enter dir + parent, enter file |
| pane/operations | 6 | Copy file, copy dir, move, delete, rename, mkdir |
| pane/selection | 2 | Glob matching, selection ops (toggle/all/invert) |
| platform | 3 | File size formatting, permissions, path display |
| preview | 3 | Binary detection, text preview, binary preview |
| viewer | 1 | Nonexistent file handling |
| **Total** | **27** | |

### Phase Completion

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | Skeleton + Dual Pane Display | Done |
| 2 | Normal Mode Navigation | Done |
| 3 | Select Mode | Done |
| 4 | Space Mode + File Operations | Done |
| 5 | Goto + Command + Config + Finder | Done |
| 6 | Preview Mode + File Viewer | Done |

### Public API Surface

**Structs (10):**
App, Config, GeneralConfig, ColorConfig, FinderMatch, Finder, PreviewState, PreviewLine, FileEntry, PaneState

**Enums (8):**
NcError, Mode, InputKind, Action (~50 variants), SortBy, SortDirection, PaneId, Dialog

### What's Not Yet Implemented

These were explicitly deferred from v0.1:

- **Archive support** — ZIP/TAR browsing (geekcommander feature, excluded by design)
- **Bookmark persistence** — Config supports bookmarks, no UI for save/manage yet
- **Shell command execution** — `:shell` command parsed but not wired to subprocess
- **Diff panes** — `:diff` command recognized but not implemented
- **Progress dialog** — Dialog enum exists, no progress bar for long file operations
- **Clipboard integration** — `y`/`p` use internal register, not system clipboard
- **Mouse support** — Terminal mouse events not handled
- **Configurable keybindings** — Config struct ready, keys are currently hardcoded
- **Undo** — No undo for file operations

### Comparison: geekcommander → ncoxide

| Aspect | geekcommander | ncoxide |
|--------|---------------|---------|
| Lines | 3,626 | 3,131 |
| Tests | 40 | 27 |
| TUI framework | tui 0.19 (deprecated) | ratatui 0.29 |
| Interface | F-keys, flat dispatch | 7 modal modes, hjkl |
| Config format | INI + TOML | TOML only |
| Platform | Windows + Linux | Linux only |
| Archive support | ZIP + TAR | None (by design) |
| Syntax preview | No | Yes (syntect) |
| Fuzzy finder | No | Yes (nucleo-matcher) |
| Date/time | chrono | jiff |
| Error handling | thiserror 1 | thiserror 2 |
| Rust edition | 2021 | 2024 |
