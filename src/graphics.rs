//! Terminal graphics: which image protocol to draw with, and how to find out.
//!
//! ratatui-image guesses from env vars, then queries the terminal through
//! stdin/stdout (kitty query, DA1 for sixel, `CSI 16 t` for the cell size).
//! That works when ncoxide talks to the terminal directly; two situations
//! need help (background in `docs/image-preview-plan.md`):
//!
//! - **zellij** implements Kitty and Sixel itself and never passes the iTerm2
//!   sequence through, while its panes inherit env vars from the zellij
//!   server — often a stale `WEZTERM_EXECUTABLE`. Those hints make
//!   ratatui-image skip the kitty/sixel query and pick iTerm2, which zellij
//!   drops: a blank pane. Inside zellij the hints are hidden for the query.
//! - **SSH** strips the env hints and may lose the cell-size reply, after
//!   which ratatui-image degrades to half-blocks even when it detected a
//!   protocol. `[preview] images` forces a protocol and `image_font_size`
//!   restores the pixel mapping.

use std::env;
use std::ffi::OsString;
use std::fmt::{self, Write as _};
use std::io::IsTerminal;

use ratatui_image::FontSize;
use ratatui_image::picker::{Capability, Picker, ProtocolType};

use crate::config::PreviewConfig;

/// Per-shell override of `[preview] images` (the same config file is often
/// used both locally and over SSH, where the right answer differs).
pub const IMAGES_ENV: &str = "NCOXIDE_IMAGES";

/// How images should be drawn, from config or [`IMAGES_ENV`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageMode {
    /// Query the terminal and use the best protocol it reports.
    Auto,
    /// Never decode: image files show their header facts only.
    Off,
    /// Unicode half-blocks, no query (works everywhere, low resolution).
    Halfblocks,
    Sixel,
    Kitty,
    Iterm2,
}

impl ImageMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "auto" => ImageMode::Auto,
            "off" | "false" | "no" | "none" => ImageMode::Off,
            "halfblocks" | "blocks" => ImageMode::Halfblocks,
            "sixel" => ImageMode::Sixel,
            "kitty" => ImageMode::Kitty,
            "iterm2" | "iterm" => ImageMode::Iterm2,
            _ => return None,
        })
    }

    /// The env override wins over the config value; an unknown value in
    /// either is logged and skipped rather than turning images off.
    pub fn resolve(config_value: &str, env_value: Option<&str>) -> Self {
        let candidates = [
            (IMAGES_ENV, env_value),
            ("[preview] images", Some(config_value)),
        ];
        for (source, value) in candidates {
            let Some(value) = value else { continue };
            match Self::parse(value) {
                Some(mode) => return mode,
                None => log::warn!("{source}: unknown image mode {value:?}, ignored"),
            }
        }
        ImageMode::Auto
    }

    /// The protocol this mode forces, if it names one.
    fn forced(self) -> Option<ProtocolType> {
        match self {
            ImageMode::Sixel => Some(ProtocolType::Sixel),
            ImageMode::Kitty => Some(ProtocolType::Kitty),
            ImageMode::Iterm2 => Some(ProtocolType::Iterm2),
            ImageMode::Auto | ImageMode::Off | ImageMode::Halfblocks => None,
        }
    }
}

impl fmt::Display for ImageMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ImageMode::Auto => "auto",
            ImageMode::Off => "off",
            ImageMode::Halfblocks => "halfblocks",
            ImageMode::Sixel => "sixel",
            ImageMode::Kitty => "kitty",
            ImageMode::Iterm2 => "iterm2",
        })
    }
}

/// Env vars ratatui-image reads as terminal-identity hints. Hidden during
/// the query inside zellij (see the module docs).
const HINT_VARS: &[&str] = &[
    "WEZTERM_EXECUTABLE",
    "TERM_PROGRAM",
    "KONSOLE_VERSION",
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
];

pub fn in_zellij() -> bool {
    env::var_os("ZELLIJ").is_some()
}

/// Run ratatui-image's stdio query. Returns `None` when stdin/stdout are not
/// a terminal (the query would block on a closed pipe) or the query fails.
///
/// Must run before any worker thread exists: inside zellij it edits the
/// process environment around the query.
fn query() -> Option<Picker> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        log::info!("images: stdio is not a terminal, skipping the graphics query");
        return None;
    }
    let hidden: Vec<(&str, OsString)> = if in_zellij() {
        HINT_VARS
            .iter()
            .filter_map(|k| env::var_os(k).map(|v| (*k, v)))
            .collect()
    } else {
        Vec::new()
    };
    if !hidden.is_empty() {
        log::info!(
            "images: inside zellij, hiding {:?} for the terminal query",
            hidden.iter().map(|(k, _)| *k).collect::<Vec<_>>()
        );
    }
    // SAFETY: called from `App::run` / `--probe-terminal` before any thread
    // is spawned, so no other thread can read the environment concurrently.
    unsafe {
        for (k, _) in &hidden {
            env::remove_var(k);
        }
    }
    let result = Picker::from_query_stdio();
    // SAFETY: as above; the query itself spawns a reader thread but joins or
    // abandons it before returning, and it does not touch the environment.
    unsafe {
        for (k, v) in &hidden {
            env::set_var(k, v);
        }
    }
    match result {
        Ok(picker) => Some(picker),
        Err(e) => {
            log::warn!("images: terminal query failed: {e}");
            None
        }
    }
}

/// The protocol the terminal actually reported support for, best first.
fn best_capability(caps: &[Capability]) -> Option<ProtocolType> {
    if caps.iter().any(|c| matches!(c, Capability::Kitty)) {
        Some(ProtocolType::Kitty)
    } else if caps.iter().any(|c| matches!(c, Capability::Sixel)) {
        Some(ProtocolType::Sixel)
    } else {
        None
    }
}

/// Outcome of [`probe`]: the picker to draw with (if any) and how it was
/// arrived at, for the log and `--probe-terminal`.
pub struct Probe {
    pub mode: ImageMode,
    pub picker: Option<Picker>,
    pub source: &'static str,
}

/// Decide how images are drawn. Runs the terminal query for every mode
/// except `off` and `halfblocks`, then applies the config's font-size and
/// protocol overrides. See the module docs for the constraints on *when*
/// this may be called.
pub fn probe(config: &PreviewConfig) -> Probe {
    let env_value = env::var(IMAGES_ENV).ok();
    let mode = ImageMode::resolve(&config.images, env_value.as_deref());
    match mode {
        ImageMode::Off => Probe {
            mode,
            picker: None,
            source: "off",
        },
        ImageMode::Halfblocks => Probe {
            mode,
            picker: Some(Picker::halfblocks()),
            source: "halfblocks, no query",
        },
        ImageMode::Auto | ImageMode::Sixel | ImageMode::Kitty | ImageMode::Iterm2 => {
            let Some(mut picker) = query() else {
                return Probe {
                    mode,
                    picker: None,
                    source: "no terminal",
                };
            };
            let mut source = "terminal query";
            if let Some([w, h]) = config.image_font_size
                && w > 0
                && h > 0
            {
                // A query that found a protocol but no cell size comes back
                // as half-blocks; the configured size restores the protocol
                // the terminal reported.
                let detected =
                    best_capability(picker.capabilities()).unwrap_or(picker.protocol_type());
                #[allow(deprecated)]
                let mut sized = Picker::from_fontsize(FontSize::new(w, h));
                sized.set_protocol_type(detected);
                picker = sized;
                source = "terminal query + configured font size";
            }
            if let Some(forced) = mode.forced() {
                picker.set_protocol_type(forced);
                source = "forced by config";
            }
            Probe {
                mode,
                picker: Some(picker),
                source,
            }
        }
    }
}

/// One-line summary for the log.
pub fn describe(probe: &Probe) -> String {
    match &probe.picker {
        None => format!("mode {} → no graphics ({})", probe.mode, probe.source),
        Some(p) => format!(
            "mode {} → {:?}, cell {}x{} px ({})",
            probe.mode,
            p.protocol_type(),
            p.font_size().width,
            p.font_size().height,
            probe.source
        ),
    }
}

/// Human-readable report for `ncoxide --probe-terminal`: the env hints, the
/// resolved mode, and what the query found. Explains the terminal matrix in
/// `docs/image-preview-plan.md` for the terminal at hand.
pub fn report(config: &PreviewConfig) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "ncoxide terminal graphics probe");
    for var in [
        "TERM",
        "TERM_PROGRAM",
        "WEZTERM_EXECUTABLE",
        "KITTY_WINDOW_ID",
        "ZELLIJ",
        "TMUX",
        "SSH_CONNECTION",
    ] {
        let shown = match env::var(var) {
            Ok(v) if var == "SSH_CONNECTION" || var == "WEZTERM_EXECUTABLE" => {
                format!("(set, {} chars)", v.len())
            }
            Ok(v) => format!("{v:?}"),
            Err(_) => "unset".to_string(),
        };
        let _ = writeln!(out, "  {var:<20} {shown}");
    }
    let env_value = env::var(IMAGES_ENV).ok();
    let _ = writeln!(
        out,
        "  [preview] images = {:?}, {IMAGES_ENV} {}",
        config.images,
        env_value
            .as_deref()
            .map_or("unset".to_string(), |v| format!("= {v:?}"))
    );
    if let Some([w, h]) = config.image_font_size {
        let _ = writeln!(out, "  [preview] image_font_size = [{w}, {h}]");
    }

    let probe = probe(config);
    let _ = writeln!(out, "result: {}", describe(&probe));
    if let Some(p) = &probe.picker {
        let _ = writeln!(out, "  capabilities: {:?}", p.capabilities());
        if p.protocol_type() == ProtocolType::Halfblocks
            && let Some(best) = best_capability(p.capabilities())
        {
            let _ = writeln!(
                out,
                "  note: the terminal reported {best:?} but no cell size; set \
                 [preview] image_font_size = [width_px, height_px] to use it"
            );
        }
        if in_zellij() && p.protocol_type() == ProtocolType::Iterm2 {
            let _ = writeln!(
                out,
                "  note: iTerm2 inside zellij shows nothing; use kitty or sixel"
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_modes() {
        assert_eq!(ImageMode::parse("auto"), Some(ImageMode::Auto));
        assert_eq!(ImageMode::parse(" Off "), Some(ImageMode::Off));
        assert_eq!(ImageMode::parse("halfblocks"), Some(ImageMode::Halfblocks));
        assert_eq!(ImageMode::parse("SIXEL"), Some(ImageMode::Sixel));
        assert_eq!(ImageMode::parse("kitty"), Some(ImageMode::Kitty));
        assert_eq!(ImageMode::parse("iterm2"), Some(ImageMode::Iterm2));
        assert_eq!(ImageMode::parse("chafa"), None);
    }

    #[test]
    fn test_resolve_env_wins_and_unknown_values_are_skipped() {
        assert_eq!(ImageMode::resolve("auto", Some("kitty")), ImageMode::Kitty);
        assert_eq!(ImageMode::resolve("sixel", None), ImageMode::Sixel);
        // A bad env value falls through to the config, a bad config to auto.
        assert_eq!(ImageMode::resolve("sixel", Some("bogus")), ImageMode::Sixel);
        assert_eq!(ImageMode::resolve("bogus", None), ImageMode::Auto);
    }

    #[test]
    fn test_best_capability_prefers_kitty() {
        assert_eq!(
            best_capability(&[Capability::Sixel, Capability::Kitty]),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            best_capability(&[Capability::Sixel]),
            Some(ProtocolType::Sixel)
        );
        assert_eq!(best_capability(&[Capability::CellSize(None)]), None);
    }

    #[test]
    fn test_probe_off_and_halfblocks_need_no_terminal() {
        let mut config = PreviewConfig {
            images: "off".into(),
            ..Default::default()
        };
        let result = probe(&config);
        assert!(result.picker.is_none());
        assert_eq!(result.mode, ImageMode::Off);

        config.images = "halfblocks".into();
        let result = probe(&config);
        assert_eq!(
            result.picker.as_ref().map(|p| p.protocol_type()),
            Some(ProtocolType::Halfblocks)
        );
        assert!(describe(&result).contains("halfblocks"));
    }

    #[test]
    fn test_report_without_terminal_does_not_hang() {
        // Under `cargo test` stdin is not a terminal: the query is skipped,
        // and the report says so instead of waiting on a closed pipe.
        let config = PreviewConfig::default();
        let text = report(&config);
        assert!(text.contains("ncoxide terminal graphics probe"), "{text}");
        assert!(text.contains("no graphics"), "{text}");
    }
}
