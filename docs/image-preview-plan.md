# Image preview via ratatui-image — implementation plan

Status: released as 0.5.0 on master, 2026-09-14 (mindtask tasks 166, 167).
Phases 0–3 are complete with the gate green (fmt, clippy `-D warnings`,
123 tests, audit); the first real-terminal check (WezTerm over SSH) found
the Kitty-placeholder problem and led to the XTVERSION rule. The rest of
the QA matrix at the end of this file is still open.

Goal: when the cursor is on an image file, the preview pane (and `Space v`)
shows the picture instead of `[Binary file]`, using the best graphics
protocol the terminal offers and falling back to unicode half-blocks.

## Findings that shape the plan

- **ratatui-image ≥ 10 requires ratatui 0.30.1**; the last release for
  ratatui 0.29 is 9.0.0. ncoxide is on 0.29. A scratch build of the current
  tree against ratatui 0.30.2 compiled with **zero source changes** and a
  clean `clippy -D warnings`, so the upgrade is a Cargo.toml bump.
- **syntect-tui is dead weight.** Nothing in `src/` references it (highlight.rs
  converts syntect styles by hand), but it silently supplies syntect's
  `default-syntaxes` feature. Dropping it requires adding that feature to our
  own syntect dependency, otherwise `SyntaxSet::load_defaults_newlines` is gone.
  syntect-tui 3.0.6 also pins ratatui ^0.29 and would block 0.30 anyway.
- ratatui-image's default features pull **libchafa via pkg-config** (a native
  library, needed at build and run time). That breaks `cargo install` on a
  clean machine and the single-binary house style: use
  `default-features = false, features = ["crossterm", "serde"]`.
- `image` with a curated format list (png, jpeg, gif, webp, bmp, ico, tiff,
  qoi; no rayon) keeps the tree small: 148 → 204 crates. Binary size can't be
  measured until real code links the crates (dead code is stripped).
- `Picker::from_query_stdio()` writes a kitty query, DA1 (sixel), `CSI 16 t`
  (cell size in px) and DSR to the terminal and reads the replies from stdin
  with a 2 s timeout. It must run **once, in raw mode, before the event loop
  touches stdin**, i.e. in `App::run` right after `EnterAlternateScreen`.
- If the cell-size query gets no answer, ratatui-image falls back to a
  TIOCGWINSZ ioctl, and if that yields nothing it **silently degrades to
  halfblocks even when a protocol was detected**. This is the most likely
  "why no picture?" cause inside multiplexers and over SSH.

## Terminal matrix (what to expect, verify with `--probe-terminal`)

| Situation | Protocol picked | Notes |
|---|---|---|
| WezTerm, local | iTerm2 | Detected from `WEZTERM_EXECUTABLE` / `TERM_PROGRAM`. ratatui-image blacklists Kitty and Sixel on WezTerm on purpose: WezTerm's kitty implementation is non-conformant and its sixel has glitches; iTerm2 is the only "bug-free" path there. |
| WezTerm over SSH | iTerm2 | The env hints don't cross SSH, but the XTVERSION reply (`CSI > q`) does: ncoxide asks for it first and applies the same rule as locally. Seen in practice before that rule existed: WezTerm answered the Kitty query, ratatui-image drew with Kitty unicode placeholders, WezTerm rendered them as missing-glyph boxes plus a "no fonts contain glyphs" warning. |
| Alacritty 0.17 | Halfblocks | Alacritty implements no graphics protocol at all (only the ayosec fork has sixel). Halfblocks give 1×2 "pixels" per cell, so a 40×20 pane is a 40×40 thumbnail. Good enough to recognise a photo, not more. |
| zellij 0.45 (any host) | Kitty or Sixel, never iTerm2 | zellij 0.45 implements Kitty graphics and Sixel itself and only advertises them if the host terminal supports them. It does **not** pass OSC 1337 (iTerm2) through. Panes inherit the zellij server's env, so a stale `WEZTERM_EXECUTABLE` makes ratatui-image choose iTerm2 → blank. Rule in our probe: if `ZELLIJ` is set, ignore env hints and force the io-query result (or the config override). |
| Dan's current setup: laptop → SSH → workstation → zellij | Sixel most likely | No env hints (checked: `TERM_PROGRAM` empty, `SSH_CONNECTION` set, zellij 0.45.1). Cell size comes from the ioctl if the SSH client forwarded pixel dimensions; if not, halfblocks — that is what `image_font_size` is for. |
| kitty, ghostty, foot | Kitty / Kitty / Sixel | Reference implementations; not installed here, listed for completeness. |

Two more protocol-independent points:

- **Overlays.** Sixel and Kitty pixels are not part of ratatui's cell buffer.
  The Space menu, dialogs, help and finder are drawn over the preview area;
  on several terminals the old pixels stay visible under them. Rule: while
  any overlay is up, render the text placeholder instead of the image. The
  image comes back on the next frame after the overlay closes.
- **Re-transmission.** With the stateless `Image` widget the whole escape
  sequence lives in one cell; ratatui's diff re-sends it only when that cell
  changes (overlay closed, resize, full repaint after `$EDITOR`). Kitty/sixel
  payloads for a half-screen pane are tens to hundreds of KB; acceptable,
  but keep the payload-producing work off the UI thread.
- **Last-row sixel scroll** (ratatui-image #57): never let an image touch the
  bottom terminal row. The status line already guarantees that in the pane;
  the viewer keeps its footer row.

## Design

Fits the existing architecture: immutable `ui::draw(&App)`, background
workers polled from the 50 ms loop with the `dirty` flag, `PreviewState`
as the single preview model.

- `PreviewKind::Image { meta: ImageMeta, decoded: Option<Arc<DynamicImage>>,
  proto: Option<Protocol>, generation: u64 }`. `ImageMeta` = dimensions,
  format, byte size, for the title (`[PREVIEW] cat.jpg — 4032×3024 JPEG · 2.1 MB`).
- Detection in `load_preview`: extension allowlist, then
  `ImageReader::open(..).with_guessed_format()` confirms by magic bytes.
  Anything that fails falls through to the existing binary/text path, so
  a `.png` that is really text still previews as text.
- **Worker** (`preview/image.rs`, same shape as `LineIndex` / `FinderWalk`):
  one long-lived thread, `Sender<ImageJob>` / `Receiver<ImageResult>`.
  A job is `(generation, path, target: Size, picker: Picker)`; the worker
  decodes with `image::Limits` (max 16384² px, 256 MiB alloc), applies the
  EXIF orientation, encodes via `picker.new_protocol(img, target,
  Resize::Fit(None))` and sends the `Protocol` back. Only the newest
  generation is applied; older results are dropped. A resize re-sends the
  cached `Arc<DynamicImage>` for re-encoding only, no second decode.
- **Target size** comes from a pure `ui::layout::preview_inner(term: Size,
  active: PaneId) -> Rect` used by both the draw code and the loop, so the
  loop can request the encode without interior mutability in `App`.
- **Debounce**: start the decode only after the cursor has rested ~80 ms
  (one or two loop ticks), so holding `j` over a photo folder decodes only
  where the cursor stops. While pending, the pane shows the meta line plus
  `decoding…`.
- **Picker** lives in `App` as `Option<Picker>`; `None` in tests and when
  images are off. `App::run` probes once, logs protocol + font size, and
  applies the override. `view_file` takes `Option<&Picker>`.
- Config (`[preview]`): `images = "auto" | "off" | "halfblocks" | "sixel" |
  "kitty" | "iterm2"`, `image_font_size = [w, h]` (escape hatch when the
  cell-size query fails), `image_max_bytes` (default 64 MiB, larger files show
  meta only). Env override `NCOXIDE_IMAGES` with the same values, for the
  SSH-vs-local case without editing the config.
- CLI: `ncoxide --probe-terminal` prints the detected protocol, cell size,
  capabilities and the env hints it saw, then exits. This is the tool for
  the matrix above and for bug reports.

## Phases

0. **Deps** (one commit): ratatui 0.30, remove syntect-tui, add
   `default-syntaxes` to syntect, add ratatui-image 11 (`crossterm`, `serde`)
   and image 0.25 (curated formats). Gate: fmt, clippy, build, test, audit.
1. **Probe + config**: `Picker` in `App::run`, config keys, env override,
   zellij rule, `--probe-terminal`, logging. Tests for the override mapping.
2. **Pane preview**: `PreviewKind::Image`, detection, worker, layout helper,
   debounce, resize re-encode, overlay rule, title meta. Tests: detection
   (magic vs extension mismatch), worker round-trip on a generated 2×2 PNG
   with `Picker::halfblocks()`, TestBackend render shows half-block glyphs
   in the pane, image hidden while help is open, stale generation dropped.
3. **Viewer + docs + QA**: full-screen fit in `view_file`, README feature
   bullet + config table, design.md module list, then the manual QA matrix
   (alacritty, WezTerm local, WezTerm over SSH, zellij on both) recorded in
   this file. Release as 0.5.0.

## Manual QA matrix (pending)

Run `ncoxide --probe-terminal`, then `ncoxide` with `p` on a photo, in each
row; record the result here. Expected values from the terminal matrix above.

| Setup | Probe result | Pane preview | `Space v` | Notes |
|---|---|---|---|---|
| Alacritty 0.17, local | halfblocks | | | |
| WezTerm, local | iTerm2 | | | |
| WezTerm → SSH → workstation | iTerm2 (XTVERSION rule) — confirmed 2026-09-14: `terminal WezTerm 20260815-143815-9c04f79f` | works (Dan, 2026-09-14) | | before the rule: Kitty → placeholder boxes |
| WezTerm → SSH → zellij 0.45 | sixel if zellij passes XTVERSION through, else kitty via zellij | | | env hints hidden; check `image_font_size` if halfblocks; does zellij answer `CSI > q` itself? |
| Alacritty → SSH → zellij 0.45 | halfblocks | | | |

Also worth a look in each: overlays (`?`, Space menu) over a picture, a
resize while a picture is up, `Space e` on the image and back.

## Out of scope (file as follow-ups if wanted)

- A sextant/quadrant fallback renderer for Alacritty (chafa-quality without
  libchafa) — worthwhile since Alacritty will not gain a protocol.
- SVG (resvg) and PDF first-page rasterising.
- Animated GIF playback (first frame only in this plan).
- Kitty unicode placeholders (`StatefulImage`) — only kitty/ghostty benefit
  and neither is in use here.
