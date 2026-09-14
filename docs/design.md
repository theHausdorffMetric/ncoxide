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
| `l` / `Enter` | Enter directory / open file (`$EDITOR`) |
| `J` / `K` | Move cursor + select (extend) |
| `G` | Jump to bottom |
| `gg` | Jump to top |
| `Ctrl-d` / `Ctrl-u` | Half-page down / up |
| `PageUp` / `PageDown` | Page up / down |
| `Tab` | Toggle active pane (`←`/`→` focus left/right) |
| `/` | Filter files (substring) |
| `v` | Enter Select mode |
| `;` | Clear selection |
| `Space` | Enter Space mode (leader) |
| `g` | Enter Goto mode |
| `:` | Enter Command mode |
| `.` | Toggle hidden files |
| `p` / `P` | Toggle preview pane |
| `r` | Quick rename (Input mode) |
| `d` | Delete (with confirm dialog) |
| `y` | Copy to other pane |
| `?` | Help overlay |
| `q` | Quit |

There are no yank/paste registers: `y` copies straight to the other pane,
Norton-Commander style. (An earlier draft of this keymap had `y`/`p`
registers; the implementation deliberately went dual-pane-native instead.)

#### Select Mode (`v`)
| Key | Action |
|-----|--------|
| `j` / `k` | Move + extend selection |
| `v` | Toggle individual |
| `a` | Select all |
| `n` | Invert selection |
| `;` | Clear selection |
| `*` | Select by glob pattern |
| `d` / `y` | Delete / copy selected to other pane |
| `Space` | Enter Space mode with selection |
| `Esc` | Return to Normal — the selection **persists** (Helix semantics; decided 2026-07-13, see docs/review-0.3.0.md R12). `;` is the explicit clear, mirroring Helix's collapse_selection. |

#### Space Mode (leader menu popup)
| Key | Action |
|-----|--------|
| `c` | Copy to other pane |
| `m` | Move to other pane |
| `d` | Delete |
| `r` | Rename |
| `n` | New directory |
| `e` | Edit with $EDITOR |
| `v` | View file (built-in pager) |
| `f` | Fuzzy file finder |
| `s` | Sort menu (via command line) |
| `i` | File info |
| `?` | Show all commands |

(Bookmarks are reached via Goto `1`-`9`, not a Space entry.)

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
| `:filter <pattern>` | Filter visible files (no pattern clears) |
| `:cd <path>` | Change directory (no path: home) |
| `:set show_hidden` | Toggle setting |

(`:shell` remains unimplemented — see "What's Not Yet Implemented".)

#### Preview Mode (`p` / `P` toggle)
- Repurposes inactive pane to show syntax-highlighted file content
- Directories preview their contents as a listing (dirs first, hidden
  filter respected, capped at 1000 entries with an explicit tail line)
- Cursor movement in file pane auto-updates preview; file operations and
  external edits invalidate it
- Focus on preview pane (Tab): Up/Down scrolls content
- `p` again restores inactive pane to directory listing
- Image files (png, jpeg, gif, webp, bmp, ico, tiff, qoi) show their header
  facts at once and the picture as soon as a worker thread has decoded and
  encoded it for the pane's cell size (Sixel / Kitty / iTerm2, half-blocks
  as the fallback); `Space v` on an image opens it full-screen. The
  protocol comes from a one-time terminal probe in `App::run`, overridable
  in `[preview]` / `NCOXIDE_IMAGES`; `--probe-terminal` explains the result.
  While an overlay is up the picture yields to its text lines (pixels are
  not in ratatui's buffer). See `docs/image-preview-plan.md`.

### Module Structure

```
ncoxide/src/
├── main.rs              — CLI args (clap), logging (fern), App::run()
├── lib.rs               — Module declarations
├── error.rs             — NcError enum (thiserror 2)
├── config.rs            — TOML config (~/.config/ncoxide/config.toml)
├── app.rs               — App struct, event loop, mode dispatch, action handlers
├── finder.rs            — Fuzzy file finder (nucleo-matcher)
├── graphics.rs          — Image protocol probe (ratatui-image Picker), zellij/SSH rules, --probe-terminal report
├── preview/
│   ├── mod.rs           — PreviewState (Loaded / Windowed / Image kinds), loaders
│   ├── highlight.rs     — syntect highlighting
│   ├── window.rs        — windowed reader for large files
│   ├── index.rs         — background line index
│   ├── search.rs, filter.rs — in-file search, fuzzy line filter
│   └── image.rs         — image probe, limited decode, decode/encode worker
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
| toml | 1.0 | Config file format |
| clap | 4 | CLI argument parsing |
| jiff | 0.2 | Date/time (consistent with qloxide) |
| walkdir | 2 | Recursive directory traversal |
| dirs | 6 | XDG directory paths |
| log + fern | 0.4 / 0.7 | Logging |
| libc | 0.2 | statvfs for disk space |
| unicode-width | 0.2 | Terminal column widths |
| nucleo-matcher | 0.3 | Fuzzy find (helix's engine) |
| syntect | 5 | Syntax highlighting (own style conversion; `default-syntaxes` feature is required) |
| ratatui-image | 11 | Image preview: Sixel / Kitty / iTerm2 / half-block rendering, terminal probe |
| image | 0.25 | Image decoding (png, jpeg, gif, webp, bmp, ico, tiff, qoi) |
| regex + memchr | 1 / 2 | Viewer in-file search (smart-case, literal fast path) |

### Architectural Decisions

1. **Elm Architecture (TEA):** `update(state, event) → state` + `view(state) → frame`. No per-frame cloning.
2. **Mode dispatch via enum + match:** Each mode returns an `Action` enum variant. App dispatches actions to pane/operations.
3. **No archive support:** Clean break from geekcommander. If added later, separate module behind feature flag.
4. **TOML config** at `~/.config/ncoxide/config.toml` (XDG compliant).
5. **Linux-only:** No Windows/winapi. Direct Unix APIs. Simplifies platform.rs.
6. **Edition 2024:** Rust 2024 edition with its stricter borrowing rules.
7. **Helix alignment (decided 2026-07-13):** when an interaction-design
   question is ambiguous, resolve it the way Helix does — not Vim, not
   Midnight Commander. Applied so far: Esc keeps the selection and `;`
   clears it (collapse_selection analog); search wraps at file ends.

---

## 3. Implementation Status

### Build Status (as of the 0.3.x review pass, 2026-07-14)

| Metric | Status |
|--------|--------|
| `cargo fmt` + `cargo clippy -D warnings` + build + test | Clean |
| `cargo test` | **101 passed**, 0 failed (+2 `--ignored` large-file proofs) |
| Edition | Rust 2024 |
| Location | `~/dev/ideas/sourcehut/ncoxide/` |
| Origin | `https://github.com/theHausdorffMetric/ncoxide` (dev and release in one repo since 2026-09-13) |
| Upstream | — (the sourcehut release mirror was retired 2026-09-13) |
| Tracker | GitHub issues on the repo (todo.sr.ht `~danprobst/ncoxide-dev` retired) |

### Code Statistics

| Component | Lines |
|-----------|------:|
| Core (app, config, error, lib, main) | 1,910 |
| Modes (mod, normal, select, space, goto, command, input) | 362 |
| Pane (state, navigation, selection, operations) | 1,120 |
| UI (draw, pane_view, status_line, dialog, help) | 676 |
| Platform + Preview + Viewer + Finder | 2,841 |
| **Total (incl. tests)** | **6,909** |

Tests are unit tests per module plus end-to-end tests that drive real key
sequences through `App::handle_key` and render to ratatui's `TestBackend`.

### Phase Completion

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | Skeleton + Dual Pane Display | Done |
| 2 | Normal Mode Navigation | Done |
| 3 | Select Mode | Done |
| 4 | Space Mode + File Operations | Done |
| 5 | Goto + Command + Config + Finder | Done |
| 6 | Preview Mode + File Viewer | Done |
| 7 | 0.3.0 review pass S1–S11 (`docs/review-0.3.0.md`): data-loss guard, hidden-root finder fix, panic hygiene, Helix selection semantics, dir-contents preview, background finder walk, viewport rendering, cleanup | Done |
| 8 | Image preview via ratatui-image (`docs/image-preview-plan.md`): ratatui 0.30, terminal probe + config + `--probe-terminal`, pane preview with off-thread decode, full-screen image view | Done (branch `image-preview`, 2026-09-14) |

### Public API Surface

**Structs:** App, Config, GeneralConfig, ColorConfig, Finder, FinderMatch,
FinderWalk, PreviewState, PreviewLine, LineIndex, LineFilter, FilterMatch,
Search, FileEntry, PaneState

**Enums:** NcError, Mode, InputKind, Action, SortBy, SortDirection, PaneId,
Dialog, ConfirmAction, SearchKind

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
| Lines | 3,626 | 6,909 (incl. tests) |
| Tests | 40 | 101 |
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
