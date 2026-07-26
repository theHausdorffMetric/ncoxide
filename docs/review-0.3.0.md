# Code & architecture review — ncoxide 0.3.0 (2026-07-13)

Full-source review (~5.6k LOC) at commit `84bf937`. Findings R1–R22 below,
grouped into commit-sized implementation steps S1–S10 at the end. Work state is
tracked in the monorepo's mindtask (`.mindtask.json`, concept "ncoxide") —
one task per step.

**Workflow per step:** implement → add/extend the listed regression tests →
gate (`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`)
→ commit with explicit pathspecs → mark the mindtask task done.

Gate status at review time: clippy clean, 70/70 tests pass, **`cargo fmt
--check` fails** (two diffs in `app.rs`, see R17).

Verified = reproduced empirically during the review, not just read from code.

---

## Findings

### Critical

#### R1 — Copy onto itself truncates files to zero bytes (VERIFIED, data loss)

`operations::copy_to` (`src/pane/operations.rs:26`) ends in
`fs::copy(source, &dest)` with `dest = target_dir.join(file_name)`. When the
other pane's cwd is the source's own directory — **the default startup state,
both panes open in cwd** — `dest == source`. `std::fs::copy` opens the
destination with truncate before reading, so the file is wiped. Verified:
`fs::copy("a.txt", "a.txt")` returns `Ok(0)` and leaves the file empty.

Repro: `ncoxide` → cursor on any file → `y` → "Overwrite …?" → `y` → file is
now 0 bytes.

Variants, same root cause:
- Copying a **directory onto itself** truncates every file inside via
  `copy_dir_recursive` (`operations.rs:31`).
- Copying a directory **into its own subtree** (other pane inside source)
  recurses into the tree it is creating — unbounded.
- `move_to`'s cross-device fallback (`operations.rs:83-92`) is copy+delete and
  inherits all of the above; same-fs `rename` onto itself is a harmless no-op.

Fix: before any prompt, canonicalize source and target dir; refuse (error
dialog) when `dest == source` or `dest` is inside `source` — same behavior as
`cp`. Guard belongs in `copy_to`/`move_to` themselves, not only in the app
layer.

Acceptance: tests for (a) file copy when both panes share a cwd → error, file
intact; (b) dir copy onto itself → error, contents intact; (c) dir copy into
own subdir → error; (d) same for move.

### High

#### R2 — Fuzzy finder returns nothing inside hidden directories (VERIFIED)

`src/finder.rs:47-52`: `filter_entry(|e| !name.starts_with('.'))` also applies
to the walk **root**, so opening the finder in `~/.config` (any dot-directory)
yields zero results for every query. Verified: 0 results under a hidden root,
1 under an identical visible root.

Fix: `e.depth() == 0 || !hidden(e)`.
Acceptance: test — finder rooted in a hidden dir finds its files.

#### R3 — Overlays panic on tiny terminals (u16 underflow)

Raw `u16` subtraction on terminal dimensions:
- `src/ui/dialog.rs:48-49` — `area.width - 4`, `area.height - 2`
- `src/ui/help.rs:9-10` — `area.width - 4`, `area.height - 4`
- `src/ui/mod.rs:141` (finder overlay) — `area.height - 2`

On a terminal < 4 rows/cols with the overlay open: panic in debug, wrap to a
huge Rect (then ratatui out-of-bounds) in release. The existing extreme-size
test (`app.rs:1197`) renders with **no** overlay open — exactly the gap.

Fix: `saturating_sub` + clamp rect to area everywhere overlay geometry is
computed.
Acceptance: extend `test_render_is_panic_free_at_extreme_sizes` to render at
1×1/20×3 with help open, a dialog open, finder open, and space menu open.

#### R4 — No panic hook: a panic leaves the terminal raw in the alternate screen

`App::run` (`src/app.rs:156-173`) restores the terminal on `Ok`/`Err` only,
not on unwind. Combined with R3, a resize race can leave the user's shell
unusable.

Fix: `std::panic::set_hook` (or update-hook wrapping the default) that calls
`disable_raw_mode()` + `LeaveAlternateScreen` before printing the panic.
Install in `run()` before entering raw mode. Same consideration for
`viewer::view_file` (it manages its own raw mode).

#### R5 — Same-filesystem moves blocked by bogus disk-space check

`execute_move` (`src/app.rs:663-668`) requires free space ≥ total source size,
but the common path is `fs::rename`, which needs no space. Moving a 20 GB dir
between panes on the same disk with 10 GB free fails with "Not enough disk
space" although rename would be instant.

Fix: drop the pre-check for moves; check space only in `move_to`'s copy
fallback (or after `rename` fails with `EXDEV`).
Acceptance: unit test on `move_to` same-fs unaffected; app-level check no
longer consulted for move (or only on fallback path).

### Medium

#### R6 — syntect assets reloaded on every preview load

`src/preview/highlight.rs:14-16` calls `SyntaxSet::load_defaults_newlines()` +
`ThemeSet::load_defaults()` **per call** (commonly ~100 ms+). Runs on every
cursor move onto a new file while preview is active, and on every small-file
pager open.

Fix: cache both in `std::sync::OnceLock`. Optionally also cap eager
highlighting (first N lines, or background it) — highlighting up to
`HIGHLIGHT_MAX` = 2 MB synchronously on cursor move stutters.

#### R7 — Finder walks the tree synchronously on every keystroke

`src/app.rs:914-918` → `finder.rs:33`: full `WalkDir` (depth 8) per keystroke
on the UI thread. Under `~` or a monorepo: multi-second freeze per key.

Fix: walk once when the finder opens, in a background thread (reuse the
`LineFilter`/`LineIndex` pattern: `Arc<Mutex<Vec<…>>>` + stop flag + join on
drop), then re-score the cached list per keystroke. nucleo-matcher scoring of
a cached list is fast.

#### R8 — O(all entries) rendering every 50 ms frame; no dirty flag

The event loop redraws unconditionally every 50 ms poll timeout
(`src/app.rs:190`). `src/ui/pane_view.rs:91-148` builds a `Row` for **every**
directory entry per frame — jiff `strftime` + ~4 String allocations per entry,
20×/s, both panes — a 50k-entry dir pins a core while idle.

Fix (either or both): render only the visible slice using
`scroll_offset`/`adjust_scroll` (currently dead, see R20) instead of
delegating scrolling to `TableState`; and/or only draw when state changed
(dirty flag set by `handle_key`/`dispatch_action`, plus redraw on resize
events).

#### R9 — Ctrl-modified keys are inserted as literal characters in input modes

`src/mode/input.rs:10`, `src/mode/command.rs:10` (and finder input at
`app.rs:866`, viewer at `viewer.rs:158`) match `KeyCode::Char(c)` without
checking `key.modifiers` — Ctrl-C during a rename types `c`.

Fix: ignore chars whose modifiers contain CONTROL/ALT (SHIFT is fine).

#### R10 — `$EDITOR` with arguments breaks

`src/app.rs:222` runs `Command::new(self.editor_command())` — `EDITOR="code
-w"` or `emacsclient -t` becomes a nonexistent binary name.

Fix: split on whitespace (program + args), or run via `sh -c '… "$@"'`.

#### R11 — Silent navigation failures

`src/pane/navigation.rs:54,67,83` use `let _ = self.refresh()`. Entering an
unreadable directory changes `cwd`, clears entries, discards the error — looks
like a plausible empty directory. Inconsistent with `app::refresh_pane`, which
surfaces errors as a dialog.

Fix: either check readability before committing the `cwd` change, or propagate
the error so the app layer can show the dialog (and consider restoring the old
cwd).

#### R12 — Esc doesn't clear the selection — RESOLVED: keep persistence (Helix alignment)

`docs/design.md` Select-mode table says Esc clears selection; implementation
(`Action::ExitToNormal`, `app.rs:411`) keeps it.

**Decision (2026-07-13): ncoxide's design philosophy aligns with Helix.** In
Helix, selections persist when leaving Select mode (Esc keeps them); clearing
is an explicit action (`;` collapse_selection, `,` keep_primary_selection).
The implementation is therefore already correct — design.md's "Esc clears
selection" line is the bug. Persistence is also load-bearing with the current
keymap: Select-mode `j`/`k` toggle-while-moving, so there is no way to build a
selection and then move the cursor without leaving Select mode.

Implement in S8:
- Keep Esc as-is (exit Select mode, selection persists; refresh still clears).
- Add `;` in Normal and Select mode → `deselect_all` (the Helix
  collapse-selection analog: reduce selection to just the cursor). This also
  revives the currently-dead `deselect_all` (R20).
- Test: select several → Esc → selection intact → `;` → cleared.
- Amend design.md's Select-mode table in S10 (Esc row + new `;` row).

This tie-breaker generalizes: where interaction semantics are ambiguous,
resolve toward Helix (e.g. R21's search-wrap inconsistency — Helix search
wraps, so windowed search should too).

#### R13 — Stale preview after external edit

`update_preview` dedupes by path (`src/app.rs:893`); `run_external` refreshes
the pane but not the preview, so after `e`-editing a file the preview shows
the old content.

Fix: clear `preview_state.path` (or call a forced reload) in `run_external`.

### Low / polish

#### R14 — In-app help misdescribes a destructive key

`src/ui/help.rs:40-41`: "y — Yank (copy path)", "p — Paste". Reality: `y`
**immediately copies to the other pane** (R1 makes this worse), `p` toggles
preview. Fix the two lines; audit the rest of the overlay against
`mode/normal.rs`.

#### R15 — README quick-ref drift

`README.md` lists `m` (move), `e` (edit), `f` (finder) as top-level keys; they
exist only in the Space menu — there is no Normal-mode `f` binding at all.
Either add the bindings or fix the table.

#### R16 — design.md drift

`docs/design.md` keymap (yank/paste registers, `P` for preview, `Space-b`
bookmarks, Select-mode `p`) no longer matches the implementation; §3 stats are
stale (claims ratatui 0.30 / toml 0.8 / dirs 5 / 27 tests / 3,131 lines —
actual: ratatui 0.29, toml 1.0, dirs 6, 70 tests, ~5.6k lines).

#### R17 — fmt gate is red

`cargo fmt --check` fails. (Review initially reported two diffs in `app.rs`
from truncated output; the drift was actually repo-wide, 19 files. Resolved
by a dedicated whitespace-only `cargo fmt` commit on `review-0.3.0-fixes`
ahead of S1; clippy and all 70 tests verified unchanged after formatting.)

#### R18 — Log defaults: predictable /tmp path, always-Debug

`src/main.rs:21` defaults to `/tmp/ncoxide.log` (predictable name in a
world-writable dir — CWE-377 shape, multi-user clobber) and `main.rs:64` logs
at Debug level permanently, growing forever. Prefer `$XDG_STATE_HOME/ncoxide/` and
Info level.

#### R19 — Literal `~` fallback in config path

`src/config.rs:104`: `PathBuf::from("~/.config")` — tilde never expands.
Only reachable when `dirs::config_dir()` is None; still, fall back to
`$HOME/.config` or skip config loading.

#### R20 — Dead code inventory

- `Action::{Redraw, OpenFile, FinderSelect, FinderCursorUp, FinderCursorDown}`
  — never produced by any mode handler.
- `PaneState::scroll_offset` + `adjust_scroll` (`navigation.rs:88`) — written,
  never read by the renderer (TableState took over scrolling). Either use for
  R8 or delete.
- `PaneState::deselect_all` — only used by tests (would be used by R12).
- `Config::save` — never called; bookmarks/settings can never persist from
  inside the app.
- `NcError::{Cancelled, PermissionDenied, NotFound}` — never constructed.

#### R21 — Windowed-viewer warts

- After hitting bottom, `top_line` becomes `None` (`window.rs:158`) and the
  line number disappears until a jump to top.
- Windowed search doesn't wrap; loaded search does (`preview/mod.rs:290-315`).
- Sort comparator allocates two lowercased Strings per comparison
  (`pane/mod.rs:206`) — use `sort_by_cached_key`.

#### R22 — Test hygiene & coverage gaps

- Temp dirs in `pane/mod.rs` + `pane/operations.rs` tests lack the pid suffix
  the other test files use → collisions between concurrent test runs.
- Coverage gaps: copy/move collision+overwrite flows (a "copy when panes share
  a cwd" test would have caught R1), finder-in-hidden-root, overlays open at
  tiny sizes, `mode/*.rs` handlers mostly only covered indirectly.

---

## Feature additions

#### F1 — Directory contents in the preview pane (requested 2026-07-13)

Today `update_preview` (`src/app.rs:896-900`) renders a static
`"[Directory]"` message when the cursor is on a directory. Instead, show the
directory's contents as a listing in the preview pane.

**Design** — reuse `PreviewKind::Loaded`, no new preview backing needed
(scrolling, gutter, render, even in-preview search then work for free):

- New `preview::load_dir_preview(path: &Path, show_hidden: bool) ->
  PreviewState`:
  - `read_dir` failure → red message state ("Cannot read directory: {e}") —
    doubles as a visible hint for unreadable dirs (complements R11).
  - Entries: name (lossy), `is_dir` via `DirEntry::file_type()` (for symlinks,
    resolve dir-ness via `path.is_dir()` — mirrors `FileEntry::from_path`'s
    follow-once rule).
  - Filter dotfiles unless `show_hidden` (passed from the active pane, so the
    preview matches the pane's `.` toggle).
  - Sort dirs-first, then case-insensitive name (the pane default; honoring
    the pane's active sort_by/direction is a possible follow-up, not in scope).
  - Cap at `DIR_PREVIEW_MAX = 1000` lines; append a dim "… N more entries"
    tail line so truncation is never silent. Empty dir → dim "[empty]".
  - Styling per line: dirs `Color::Blue` + bold + trailing `/`, symlinks
    `Color::Magenta`, files default — matches the default `Theme`; plumbing
    the configured theme into the preview module is a deliberate non-goal for
    now (the preview module currently has zero config dependency; syntect
    colors are hardcoded the same way).
- `update_preview`: replace the `"[Directory]"` branch with
  `load_dir_preview(&entry.path, pane.show_hidden)`.
- **Invalidation:** clear `preview_state.path` after `run_batch`, rename, and
  mkdir so the next `update_preview` reloads — otherwise a previewed dir goes
  stale when a file op changes it (e.g. copying into the other pane while
  previewing that directory). Same mechanism as R13's fix for external edits;
  implement both in one place.

**Tests:** unit — dirs-first ordering, hidden filtering respects flag, cap +
"more" tail, empty dir, unreadable dir; integration — preview on, cursor on a
subdir → rendered frame contains child names with trailing `/`; copy a file
into a previewed dir → preview reflects it after the op.

**Scheduling decision: implement as S11, immediately after S8.** Rationale:
it must not jump the S1–S4 critical tier (data loss, broken finder, panic
hygiene); it shares its invalidation mechanism with S8/R13, so doing it right
after S8 touches `update_preview` once instead of twice; and landing before
S9 means the dirty-flag/render-perf work sees the feature's (bounded,
capped) synchronous dir read and can account for it.

---

## Implementation plan (ordered, commit-sized)

| Step | Findings | Summary |
|------|----------|---------|
| S1 | R1 | Refuse self/nested copy & move in `operations.rs` + regression tests |
| S2 | R2 | Finder hidden-root fix (`depth() == 0` guard) + test |
| S3 | R3, R4 | Panic hook restoring terminal; saturating overlay geometry; tiny-terminal overlay tests |
| S4 | R17, R14, R15 | `cargo fmt`; fix help overlay (`y`/`p`); fix README quick-ref |
| S5 | R5 | Disk-space check only on cross-device move fallback |
| S6 | R6 | `OnceLock` for SyntaxSet/ThemeSet (+ optional highlight cap) |
| S7 | R9, R10 | Ignore CTRL/ALT chars in all text inputs; `$EDITOR` arg splitting |
| S8 | R11, R13, R12 | Surface navigation refresh errors; reload preview after external edit; Helix-aligned selection semantics (Esc keeps, `;` clears) |
| S11 | F1 | Directory contents in preview pane (**executes here, after S8** — shares invalidation with R13) |
| S9 | R7, R8 | Background finder walk with cached re-scoring; visible-slice rendering and/or dirty-flag redraw |
| S10 | R16, R18, R19, R20, R21, R22 | design.md realignment; log path/level; config fallback; dead-code prune; viewer warts; test hygiene |

S1–S4 are the "stop the bleeding" tier (data loss, broken feature, crash
hygiene, misleading docs) and are each small. S5–S8 are behavioral corrections.
S11 (feature F1) executes right after S8, with which it shares the
preview-invalidation mechanism. S9 is the largest change (threading + render
path). S10 is cleanup and can be split further if convenient.

---

## Post-merge verification review (2026-07-14)

After S1–S11 merged to master, an independent multi-agent review of the full
branch diff (21 agents: per-angle finders + adversarial verifiers) surfaced
13 defects in the new code itself. Resolutions, pre-0.4.0:

**Fixed (V1–V6):**
- **V1** `operations.rs` — a destination entry that is a *symlink or hard
  link back to the source* defeated the S1 path guard; `fs::copy` truncated
  the source through it. Fixed with a device+inode same-file check (cp-style)
  when the destination exists. Tests: symlink dest, hard-link dest.
- **V2** `app.rs execute_move` — S5 removed the aggregate space check
  entirely, so an over-capacity multi-file *cross-device* move partially
  completed. Restored the aggregate pre-check for the cross-device subset of
  sources only (same-fs renames stay unchecked).
- **V3** `FinderWalk`/`LineIndex`/`LineFilter` Drop joined their worker
  thread on the UI thread; a readdir/read blocked on a dead network mount
  froze the TUI. All three now signal-and-detach, never join.
- **V4** finder progress ticks reset `finder_cursor` to 0 (arrow selection
  never stuck during a walk) and re-scored the full list every 50 ms holding
  the walk lock. Progress re-scores now preserve the cursor (clamped) and are
  throttled to ≥512 new paths or the completion transition.
- **V5** dir preview went stale on `.` (hidden toggle) — the by-path dedupe
  didn't know the preview depends on `show_hidden`. Fixed structurally:
  preview invalidation is folded into `refresh_pane`/`refresh_both`, which
  also removed the four copy-pasted refresh+invalidate pairs (V6).
- **V6** panic hook now also emits `cursor::Show` (ratatui hides the cursor
  during draw; the normal `show_cursor` cleanup can't run during an unwind).

**Accepted / deferred:**
- Windowed backward-search wrap can scan to EOF on the UI thread for
  unmatched queries. Same cost class as the pre-existing forward search from
  the top of a large file (any wrapping search must visit every line once);
  proper fix is an async/cancellable search — follow-up, not a 0.4.0 blocker.
- `accepts_text` could drop modifier-tagged composed characters (AltGr /
  macOS Option). ncoxide is Linux-only and does not enable crossterm's
  keyboard-enhancement flags, so composed chars arrive unmodified in
  practice; revisit if kitty-protocol support is ever enabled.
- Minor cleanups deferred: the two disk-space shortfall messages differ in
  wording (`app.rs` copy check vs `operations.rs` fallback check); the dir
  preview re-implements the pane's dirs-first sort; `Finder::find` is
  test-only. All cosmetic.
