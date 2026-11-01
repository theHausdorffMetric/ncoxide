# Resizable pane split — implementation plan

Status: **shipped in 0.6.0** — implemented on branch `pane-resize` and merged
2026-11-01 (gate green: fmt, clippy `-D warnings`, 151 tests, audit; mindtask
task 239 under concept `ncoxide`, done), published to crates.io and tagged
`v0.6.0` by Dan from Tuxedo the same day. The decisions below were confirmed by
Dan before phase 1. Baseline was master `4679783`, 139 tests.

Goal: let the user **move the divider between the two halves of the
screen**, and keep that choice through terminal resizes and mode changes.
Today `ui::layout::regions` hard-codes `Percentage(50) / Percentage(50)`,
so the file list and the preview always get the same width — too wide a
list when the preview is a picture, too narrow a list in a directory of
long names. Resizing is also the missing half of compare mode: the locked
and the live picture are rarely the same shape.

Scope: the **one** split ncoxide has — the vertical divider between the
left and the right half. The status line is fixed; there is no horizontal
split to resize. The pager/viewer is full-screen and untouched.

## Decisions (confirmed by Dan, 2026-11-01)

- **The divider is one number: `split`, the left half's share in percent.**
  Not cells. A ratio survives terminal resizes the way a dual-pane
  commander should (both halves grow), and it is what the config stores,
  what `:split 30` sets and what a later mouse drag would map a column to.
  Cells enter only as a floor at layout time (below).
- **Keys act on the focused half, not on the divider:** `>` makes the half
  you are looking at wider, `<` narrower, `=` equal. In plain dual-pane
  mode the focused half is the active pane. In preview mode it is the file
  list, or the preview after `Tab` (preview focus) — so `>` on a picture
  grows the picture. In compare mode the file-list half holds the locked
  picture, so `>` grows the locked shot, `Tab` `>` the live one. This is
  Vim's `Ctrl-w >` semantics ("the current window gets wider") in ncoxide's
  single-key style.
- **Keys: `<` / `>` / `=` in Normal mode, no prefix.** Helix has no resize
  bindings at all (its `Ctrl-w` window mode splits, moves and closes), so
  the Helix-alignment rule gives no answer; `Ctrl-w` as a prefix would add
  a pending-key state for three actions. `<`/`>` are Helix's indent and
  `=` its format — neither has a meaning in a file commander, and all
  three are free in `mode/normal.rs`. They are not added to Select mode
  (nothing to resize while marking files). The `Space` menu is not touched
  either: these are repeat-and-watch keys, a menu entry would defeat that.
- **Step 5 points per press**, repeat the key; no count prefix (Normal mode
  has none today, and 5 presses cover the useful range).
- **Floor per half: 16 cells** (border + a 14-char name column), enforced
  at layout time when the terminal allows it (width ≥ 32), otherwise fall
  back to the plain 50/50 the code has today. The stored percent is
  clamped to **10..=90** so an extreme value cannot hide a half.
- **`:split <N>`** sets the left half to N % directly (absolute, left —
  the natural reading of a divider position, independent of focus);
  **`:split`** alone returns to the configured value, `=` to 50.
- **Config `[layout] split = 50`** is the starting value. **Runtime changes
  are not written back:** `Config::save` would rewrite the user's file
  from the struct and lose comments and ordering, and Helix does not
  persist layout either. (If persistence is wanted later, a `:split save`
  that rewrites just that key is the shape.)
- **Narrow lists get fewer columns.** The file table asks for
  `Min(20) + 10 + 16 + 10` cells; at the widths this feature makes common
  (a 30 % list on a 120-column terminal is 36 cells) the fixed columns
  crowd out the name. Columns drop from the right: below 58 cells no
  `Perms`, below 48 no `Modified`, below 32 no `Size`. Thresholds live
  next to the widths in `pane_view.rs` and are unit-tested, not tuned by
  eye.

```
┌ ~/photos/2026-09-trip ──────────┐┌ [PREVIEW] IMG_0421.jpg — 4032×3024 JPEG · 1.9 MB ────────────────────┐
│ Name              Size          ││                                                                       │
│ IMG_0417.jpg      2.1 MB        ││                                                                       │
│ IMG_0418.jpg      2.0 MB        ││                        (picture, 70 % of the width)                   │
│>IMG_0421.jpg      1.9 MB        ││                                                                       │
└─────────────────────────────────┘└───────────────────────────────────────────────────────────────────────┘
 NOR   ~/photos/2026-09-trip
```
After `<` `<` `<` `<` from the default — list 30 %, preview 70 %;
`Modified` and `Perms` have dropped out of the list.

## Design

Fits the existing shape: the split is App state, `ui::draw(&App)` and
`pump_images` read it through `layout`, nothing else changes its contract.

- **`App.split: u16`** (left half, percent), seeded from
  `config.layout.split` in `new_with_config`. Three helpers:
  `grow_focused(step)`, `shrink_focused(step)` (resolve the focused half
  to a sign on `split`), `set_split(n)` (clamps 10..=90).
- **`layout::regions(area, split)`** replaces the hard-coded percentages:
  `left = panes.width * split / 100`, then clamped into
  `[MIN_PANE_WIDTH, panes.width - MIN_PANE_WIDTH]` when
  `panes.width >= 2 * MIN_PANE_WIDTH`, else the old 50/50. `preview_area`
  and `preview_inner` take `split` too. Every caller passes `app.split`:
  `ui::draw`, `pane_view::draw_panes` (via `Regions`), `App::pump_images`,
  and the test helpers in `app.rs` (`pump_until_encoded` and the four
  direct `preview_inner` calls).
- **Actions:** `SplitGrow`, `SplitShrink`, `SplitReset` (to 50),
  `SetSplit(Option<u16>)` (`None` = the configured value) in
  `mode::Action`; `<`/`>`/`=` in `mode/normal.rs`; `:split [N]` in
  `mode/command.rs::execute_command` (non-numeric or out-of-range →
  `Action::None`, as `:sort nonsense` is today).
- **Preview-focus intercept:** `dispatch_action`'s "preview focused" arm
  must let the four split actions through (like `TogglePreview` and
  `ToggleLock`) — that is exactly the case where `>` should grow the
  preview.
- **Images re-encode by themselves.** The encode target is
  `preview_inner(...).as_size()`; a split change gives a new target, the
  existing `target != encoded.target` rule in `pump_slot` requests a new
  encode, and `draw_preview_pane` shows the text lines until the payload
  for the new size arrives (exactly the resize path from
  `docs/image-preview-plan.md`). Repeated `>` presses are bounded by the
  worker's per-slot coalescing (newest job per slot). The live slot's
  debounce keys off `preview_changed_at`, which a split change does not
  touch — intended: the pane did not change, only its size, and the
  coalescing is the back-pressure. The compare page is unaffected (the
  browser has its own layout).
- **Responsive columns** in `pane_view::draw_single_pane`: a
  `visible_columns(width) -> &[Column]` helper picks the header and the
  `Constraint`s; row construction maps over the same list so header and
  cells cannot disagree.
- **Status line:** nothing permanent. The borders show the split. (A
  transient `30 %` while a key repeats was considered and dropped: it
  would be the first timed message in the status line and the border is
  enough feedback.)
- **Config:** `LayoutConfig { split: u16 }` with `Default` 50, `#[serde(default)]`
  like the other sections; an out-of-range value is clamped at `App`
  construction, not rejected (the rest of the config still loads).
- **Help overlay + README key table + design.md keymap** gain the three
  keys and `:split`.

### Not in this pass (follow-ups)

- **Mouse drag on the divider.** Mouse events are not read anywhere yet
  (listed under "not yet implemented" in `design.md`); enabling mouse
  capture also changes how the terminal's own text selection works, so it
  is a decision of its own. When it comes, the drag maps the column to a
  percent and calls `set_split` — nothing in this plan has to change.
- **Zoom / "only" (one half at 100 %).** In preview mode that is `p`, in
  compare mode `Space v` (full-screen image); for plain dual-pane use it
  would only hide the other listing. Not worth a key until asked for.
- **Stacked orientation** for narrow terminals — already a follow-up of
  the compare plan; `split` would then be the top half's share, same type.
- **Persisting the split** (`:split save`) — see Decisions.

## Tests (TestBackend end-to-end, like the preview tests)

- layout unit: `regions(area, 30)` gives the left 30 % of the panes
  width; `split` 5 and 95 are clamped by the cell floor on an 80-column
  terminal; a 20-column terminal is 50/50 whatever the split; tiny
  terminals (0×0, 1×1, 3×2) stay empty without a panic (extend the
  existing test).
- `>` on the left pane widens the left border position in the rendered
  frame; `<` narrows it; `=` restores the default; the right pane's
  listing is still drawn.
- preview on, `Tab` (preview focused), `>` → the preview half is wider
  than the list half in the frame; `Esc` → `>` widens the list again.
- `:split 30` sets 30 (left border at the expected column); `:split 7` and
  `:split abc` are no-ops; `:split` returns to the configured value after
  a `>`.
- repeated `<` stops at the floor: the list half never goes below 16
  cells and never disappears.
- image: after `pump_until_encoded` at 50 %, `>` changes the encode
  target and the pump delivers a payload for the new target; the old one
  is not drawn as pixels meanwhile (title still shows the facts).
- compare mode: with a locked picture both targets change on `>`, and
  both slots re-encode (the per-slot coalescing test, extended).
- columns: `visible_columns(36)` is Name + Size; `visible_columns(80)` is
  all four; a 36-cell list renders without `Perms` and `Modified` headers.
- config: `[layout] split = 30` parses; missing section → 50; `split =
  150` clamps to 90 at construction.

## Phases

1. **Layout + state.** `LayoutConfig`, `App.split`, `layout::regions`
   with the floor, all callers updated, layout unit tests. Gate: fmt,
   clippy `-D warnings`, build, test, audit.
2. **Keys + command.** Actions, `<`/`>`/`=`, `:split`, the preview-focus
   pass-through, end-to-end tests including the image and compare cases.
3. **Responsive columns.** `visible_columns`, header/rows from one list,
   tests at 36 / 48 / 58 / 80 cells.
4. **Docs + release prep.** README (feature bullet, key table, `[layout]`
   in the config example, `:split` under Command mode), `design.md`
   (keymap tables, Preview-mode section, phase 11 in the table, "mouse
   support" note pointing here), help overlay, this file's status line.
   Bump `Cargo.toml` to 0.6.0; publishing is Dan's step on Tuxedo
   (`crates.io/release.justfile`), as for 0.5.0.

Estimated size: ~250 lines of code and ~200 of tests, one branch
(`pane-resize`), merged to master when the gate is green.
