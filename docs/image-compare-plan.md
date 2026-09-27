# Lock/compare for image previews — implementation plan

Status: phases 1–3 implemented with the gate green (fmt, clippy
`-D warnings`, 129 tests, audit), 2026-09-27; mindtask task 176 under
concept `ncoxide`. The companion page (below) followed the same day (139
tests; task 177). Both were merged to master the same day. Builds on the
0.5.0 image preview (`docs/image-preview-plan.md`); since 0.5.0 was never
published, **0.5.0 ships all three** — preview, compare, page — under task
167 (Cargo.toml stays 0.5.0). Still to do before publishing: a
real-terminal check of `L` (WezTerm over SSH, as for the preview) and a
real-browser check of the page.

Goal: while flipping through a directory of pictures, **lock** the one under
the cursor and keep flipping — the locked picture stays on screen beside the
live preview, so two shots can be compared side by side without leaving the
file commander.

## Decisions (Dan, 2026-09-27)

- **Layout: the locked picture takes over the file-list half.** In preview
  mode the panes area is already the whole screen minus the status line, so
  two full-height halves are the largest two rectangles available. The
  alternative — stacking locked/live *inside* the preview pane and keeping
  the list — shows each photo at ~40% of the area (8×16 px cells, 200×50
  terminal, 4:3 photo: 784×588 px vs 491×368 px). Translucent "blending" is
  not an option anyway: Sixel/Kitty pixels live outside ratatui's cell
  buffer, so only a hard takeover renders correctly.
- **Key: `L`** in normal mode (free; `l` is enter-dir), also `Space l` in the
  menu for discoverability. `L` on an image locks it and turns the preview on
  if it was off. `L` on a *different* image re-locks to that one; `L` on the
  locked image itself unlocks. `p` (preview off) also drops the lock. Esc
  keeps the lock, consistent with "Esc keeps the selection".
- **The lock is a path, not a cursor: it survives directory changes.** Lock
  in `raw/`, walk to `out/`, compare the export against the original. If the
  locked file vanishes (deleted, moved) the lock is dropped — deleting the
  worse of two shots is part of the workflow.
- **`j`/`k` step over non-images while locked.** The mode is about pictures;
  landing on a sidecar `.xmp` beside a locked photo is noise. Only `j`/`k`
  (and the arrow keys) skip; `G`, `gg`, paging jump as usual. Mode-specific,
  deliberately not Helix-like.
- **Flying blind is mitigated, not avoided:** pane titles carry both file
  names, the status line gains a `CMP` badge and the cursor position
  (`23/240 items`). If that proves too little in practice, a narrow
  name-only sidebar on wide terminals is the fallback — not built up front.

```
┌ [LOCKED] IMG_0417.jpg — 4032×3024 JPEG · 2.1 MB ─┐┌ [PREVIEW] IMG_0421.jpg — 4032×3024 JPEG · 1.9 MB ┐
│                                                  ││                                                  │
│                 (locked picture)                 ││              (follows the cursor)                │
│                                                  ││                                                  │
└──────────────────────────────────────────────────┘└──────────────────────────────────────────────────┘
 NORMAL   ~/photos/2026-09-trip   │  CMP  │  23/240 items
```

## Design

Fits the existing shape: immutable `ui::draw(&App)`, the image worker polled
from the 50 ms loop, `PreviewState` as the single preview model.

- `App.locked_preview: Option<PreviewState>` is the whole model. Locking
  loads a fresh `PreviewState` for the path (`load_preview`, so a mislabelled
  non-image falls through and is refused by the `image::probe` gate).
- **Layout reuse:** `layout::preview_inner(area, active)` is the live target
  as before; `preview_inner(area, active.other())` is the locked target. No
  new layout variant. `pane_view::draw_panes` draws the locked state into the
  file-list half with a `[LOCKED]` title and a yellow border; the live half
  is unchanged.
- **Worker: per-slot coalescing.** `ImageWorker` used to keep only the newest
  queued job, which is right for one moving cursor but would drop one of two
  encodes after a resize (both halves need a new payload). `ImageJob` and
  `ImageResult` carry `slot: ImageSlot { Live, Locked }`; the worker keeps
  the newest job *per slot* and re-drains between encodes, so a stale
  second-slot job is replaced before it runs. Results route to the slot's
  state; the existing generation check still drops anything stale.
- `pump_images` runs the same request logic over both slots via a
  `pump_slot` helper (live debounced by `IMAGE_DEBOUNCE`, locked not — its
  target only changes on lock or resize).
- The overlay rule (text placeholder while help / dialogs / Space menu /
  finder are up) applies to both halves through the existing `show_image`
  flag.
- `invalidate_preview` (called after every pane refresh) drops the lock when
  its path no longer exists.
- `dispatch_action`'s preview-focused intercept lets `ToggleLock` through,
  like `TogglePreview`.
- `PaneState::cursor_up_where` / `cursor_down_where` move to the nearest
  entry satisfying a predicate; compare mode uses the cheap extension check
  (`image::has_image_extension`) rather than the header probe.

## Tests (TestBackend end-to-end, like the preview tests)

- lock → both halves render half-block glyphs, titles show `[LOCKED]` and
  `[PREVIEW]`; `j` skips a `.txt` between two images; `L` on the locked file
  unlocks and the file list is back.
- `L` on another image re-locks; `L` on a text file is a no-op.
- `L` with preview off turns it on; `p` drops the lock.
- lock survives `h` (parent dir); deleting the locked file + refresh drops it.
- resize re-encodes **both** slots (the coalescing regression test: with the
  old worker one slot never gets a payload and the wait times out).
- overlay hides both pictures; status line shows `CMP` while locked.
- worker unit test: Live/Locked/Live jobs → results for the newest of each
  slot arrive.

## Phases

1. Worker slot + coalescing, `has_image_extension`, `cursor_*_where`.
2. `locked_preview`, `ToggleLock`, keys, pump over both slots, rendering,
   status line, help/menu, tests. Gate: fmt, clippy `-D warnings`, build,
   test, audit.
3. Docs: README bullet + key table, `design.md` (preview-mode section, module
   notes, phase table). Ships in 0.5.0 together with the image preview.

## Companion page: the pictures in a browser (`Space w`, `:web`)

Evaluated 2026-09-27 as "serve the locked and the live picture over local
HTTP". Decision (Dan): the **companion** variant — the terminal compare
stays the default, the page is an opt-in escalation for the moments the
terminal cannot deliver: resolution, zoom, a difference overlay. Not the
"replace" variant: ncoxide's premise is staying in the terminal, and in the
main setup (laptop → SSH → workstation → zellij) the browser is on the
other machine.

Why it earns its place: a terminal half-pane is ~800×750 px at best; the
browser shows the original, and one page retires three follow-ups from the
list below (1:1 detail, orientation, a bigger viewer) plus formats the
terminal path lacks (SVG, AVIF, PDF are the browser's problem now).

### Design

- `web.rs`: a `std::net` HTTP server — no crate. Four GET routes on
  loopback do not justify one, and hand-rolling keeps every security
  property in our hands. One thread per connection, `Connection: close`
  on every response (browsers cope; polls are one TCP round trip on
  loopback), 5 s read / 30 s write timeouts, requests capped at 8 KiB.
- **Capability, not a file server:**
  - binds `127.0.0.1` only (not configurable);
  - every route lives under a random 128-bit token from `/dev/urandom`,
    new per run: `http://127.0.0.1:<port>/<token>/`;
  - exactly two picture routes, `img/locked` and `img/live`, resolving to
    the paths ncoxide currently holds — **no path parameter**, so there is
    nothing to traverse (tests try anyway);
  - `Host` must be `127.0.0.1:<port>` or `localhost:<port>` (DNS
    rebinding from a page open elsewhere is refused with 403);
  - GET/HEAD only; wrong or missing token is a 404 like any other miss.
  - Blast radius if the token leaks: whoever has it sees the two pictures
    currently shown, nothing else.
- `state` is a small hand-built JSON (`generation`, `locked`, `live` with
  name / path / facts); the page polls it every 250 ms and reloads the
  pictures only when the generation changed. `publish` bumps the
  generation only on a real change, so keystrokes that do not move the
  pictures cost nothing.
- Pictures are served as the file's own bytes with the right media type;
  TIFF and QOI (no browser support) are transcoded to PNG under the
  preview's decode limits. A file that vanished after publishing is a 404.
- **The live side follows the cursor even with the terminal preview off**
  (a header probe per keystroke, as the preview itself does): the browser
  can be the viewer while the terminal shows the full-width list.
- The page (`web/compare.html`, inlined): dark, two panes with captions;
  modes **Fit** (`f`), **1:1** (`1`, both panes scroll together), **Diff**
  (`d`, the live picture over the locked one with
  `mix-blend-mode: difference` — spots what changed between two exports).
  Single-pane layout while nothing is locked; a banner when ncoxide is gone.
- `[web] port = 6269` (fixed by default so an SSH `LocalForward` can be set
  up once — "NCOX" on a phone keypad; `0` = any free port; if taken, falls
  back to a free one and the dialog shows the actual URL) and
  `open_browser = true` (`xdg-open`, only with a local display and never
  over SSH; otherwise the URL is shown to copy — WezTerm makes it
  clickable, and with the forward it just works on the laptop).
- UI: `Space w` / `:web` start or show the URL; `:web stop` ends it; `WEB`
  badge in the status line. Dialogs now size to their content (width from
  the longest line, height from the wrapped count) so the 55-char URL stays
  on one row.

### Tests (web.rs, socket level)

Routes under the token (page, state before/after publish, both pictures
byte-identical with the right media type, HEAD, redirect to the trailing
slash, unlock → 404); everything outside the capability (no/wrong token,
favicon, unknown routes, traversal-shaped targets, wrong/missing `Host`,
POST/DELETE, garbage and oversized requests → 400 without a panic);
transcode QOI → PNG with the right dimensions, vanished file → 404 on both
paths, undecodable file → 500; drop closes the port and a fixed port is
honoured; JSON escaping; the browser rule as a pure function. App level:
`Space w` starts on a free port, the URL is in the dialog and on one row,
the state follows cursor, lock and unlock (also with the preview off),
`:web stop` closes it. Config and command parsing.

## Follow-ups (not in this pass)

- **Lock without the terminal preview.** `L` still turns the preview on,
  because the lock is drawn in its layout. With the page open one may want
  the list full-width and the browser as the only viewer; a "web-only
  compare" flag would keep the preview off and only publish.
- **Auto orientation:** on narrow/tall terminals (≲100 cols) full-width
  *stacked* halves beat side-by-side (491×368 vs 384×288 px for a 4:3 photo
  at 100×50). A `compare_regions(area, pixel_aspect)` helper could pick; adds
  a layout variant, so only if ncoxide is actually used at that width.
- **1:1 detail toggle in the terminal** (`Resize::Crop`) — superseded by the
  page's 1:1 mode for anyone with a browser at hand; keep only if the
  terminal-only case turns out to need it.
- OSC 8 hyperlink for the URL in the dialog (ratatui support to be
  verified); WezTerm's implicit URL detection covers it meanwhile.
- Swap sides: not needed, re-lock covers it.
