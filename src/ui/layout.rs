//! Screen geometry shared by the renderer and the event loop, so the loop can
//! size an image encode for exactly the cells the preview pane will draw.
//!
//! The one movable boundary is the vertical divider between the two halves:
//! `split` is the left half's share in percent (`<` / `>` / `=` / `:split`,
//! seeded from `[layout] split`). See `docs/pane-resize-plan.md`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders};

use crate::pane::PaneId;

/// Equal halves — the default and what `=` restores.
pub const SPLIT_DEFAULT: u16 = 50;
/// Bounds of the stored percent, so an extreme value can never hide a half.
pub const SPLIT_MIN: u16 = 10;
pub const SPLIT_MAX: u16 = 90;
/// One `<` / `>` press.
pub const SPLIT_STEP: u16 = 5;
/// Narrowest a half may get in cells (border + a 14-char name column) when
/// the terminal is wide enough to give both halves that much.
pub const MIN_PANE_WIDTH: u16 = 16;

/// Clamp a requested percent into `SPLIT_MIN..=SPLIT_MAX`.
pub fn clamp_split(split: u16) -> u16 {
    split.clamp(SPLIT_MIN, SPLIT_MAX)
}

/// The top-level regions of a frame.
pub struct Regions {
    /// Both panes together (above the status line).
    pub panes: Rect,
    pub status: Rect,
    pub left: Rect,
    pub right: Rect,
}

/// Width of the left half for a panes area `width` cells wide: `split`
/// percent, then the cell floor for both halves when the terminal allows
/// it; too narrow for two floors and it is plain halves whatever `split`.
fn left_width(width: u16, split: u16) -> u16 {
    let split = clamp_split(split);
    let left = (u32::from(width) * u32::from(split) / 100) as u16;
    if width >= 2 * MIN_PANE_WIDTH {
        left.clamp(MIN_PANE_WIDTH, width - MIN_PANE_WIDTH)
    } else {
        width / 2
    }
}

pub fn regions(area: Rect, split: u16) -> Regions {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let panes = rows[0];
    let left = left_width(panes.width, split);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left), Constraint::Fill(1)])
        .split(panes);
    Regions {
        panes,
        status: rows[1],
        left: cols[0],
        right: cols[1],
    }
}

/// Outer rect of the preview pane: the side the active pane does not occupy.
pub fn preview_area(area: Rect, active: PaneId, split: u16) -> Rect {
    let r = regions(area, split);
    match active {
        PaneId::Left => r.right,
        PaneId::Right => r.left,
    }
}

/// Inner rect of the preview pane (inside its border): the cells an image is
/// encoded for. Saturates to empty on tiny terminals.
pub fn preview_inner(area: Rect, active: PaneId, split: u16) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .inner(preview_area(area, active, split))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_inner_is_the_inactive_half_minus_border() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(
            preview_inner(area, PaneId::Left, SPLIT_DEFAULT),
            Rect::new(41, 1, 38, 21)
        );
        assert_eq!(
            preview_inner(area, PaneId::Right, SPLIT_DEFAULT),
            Rect::new(1, 1, 38, 21)
        );
    }

    #[test]
    fn test_preview_inner_is_empty_on_tiny_terminals() {
        for (w, h) in [(0u16, 0u16), (1, 1), (3, 2), (4, 4)] {
            for split in [SPLIT_MIN, SPLIT_DEFAULT, SPLIT_MAX] {
                let inner = preview_inner(Rect::new(0, 0, w, h), PaneId::Left, split);
                assert!(inner.width <= w && inner.height <= h, "{w}x{h}: {inner:?}");
            }
        }
    }

    #[test]
    fn test_split_percent_sets_the_left_width() {
        let area = Rect::new(0, 0, 100, 24);
        let r = regions(area, 30);
        assert_eq!(r.left, Rect::new(0, 0, 30, 23));
        assert_eq!(r.right, Rect::new(30, 0, 70, 23));
        assert_eq!(r.status, Rect::new(0, 23, 100, 1));
        // The two halves always tile the panes area exactly.
        for split in 0..=100 {
            let r = regions(area, split);
            assert_eq!(r.left.width + r.right.width, 100, "split {split}");
        }
    }

    #[test]
    fn test_split_is_clamped_to_percent_bounds() {
        assert_eq!(clamp_split(0), SPLIT_MIN);
        assert_eq!(clamp_split(7), SPLIT_MIN);
        assert_eq!(clamp_split(50), 50);
        assert_eq!(clamp_split(90), SPLIT_MAX);
        assert_eq!(clamp_split(150), SPLIT_MAX);
        // 200 columns: 10 % would be 20 cells, above the floor, so the
        // percent bound is what limits it.
        let r = regions(Rect::new(0, 0, 200, 24), 0);
        assert_eq!(r.left.width, 20);
    }

    #[test]
    fn test_cell_floor_keeps_both_halves_usable() {
        // 80 columns: 10 % = 8 cells, below the floor → 16; 90 % → 64.
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(regions(area, SPLIT_MIN).left.width, MIN_PANE_WIDTH);
        assert_eq!(regions(area, SPLIT_MAX).left.width, 80 - MIN_PANE_WIDTH);
        assert_eq!(regions(area, SPLIT_MAX).right.width, MIN_PANE_WIDTH);
        // Too narrow for two floors: plain halves whatever the split.
        let narrow = Rect::new(0, 0, 20, 24);
        for split in [SPLIT_MIN, 30, SPLIT_DEFAULT, 70, SPLIT_MAX] {
            assert_eq!(regions(narrow, split).left.width, 10, "split {split}");
        }
    }
}
