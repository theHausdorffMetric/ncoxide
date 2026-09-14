//! Screen geometry shared by the renderer and the event loop, so the loop can
//! size an image encode for exactly the cells the preview pane will draw.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders};

use crate::pane::PaneId;

/// The top-level regions of a frame.
pub struct Regions {
    /// Both panes together (above the status line).
    pub panes: Rect,
    pub status: Rect,
    pub left: Rect,
    pub right: Rect,
}

pub fn regions(area: Rect) -> Regions {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    Regions {
        panes: rows[0],
        status: rows[1],
        left: cols[0],
        right: cols[1],
    }
}

/// Outer rect of the preview pane: the side the active pane does not occupy.
pub fn preview_area(area: Rect, active: PaneId) -> Rect {
    let r = regions(area);
    match active {
        PaneId::Left => r.right,
        PaneId::Right => r.left,
    }
}

/// Inner rect of the preview pane (inside its border): the cells an image is
/// encoded for. Saturates to empty on tiny terminals.
pub fn preview_inner(area: Rect, active: PaneId) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .inner(preview_area(area, active))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_inner_is_the_inactive_half_minus_border() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(preview_inner(area, PaneId::Left), Rect::new(41, 1, 38, 21));
        assert_eq!(preview_inner(area, PaneId::Right), Rect::new(1, 1, 38, 21));
    }

    #[test]
    fn test_preview_inner_is_empty_on_tiny_terminals() {
        for (w, h) in [(0u16, 0u16), (1, 1), (3, 2), (4, 4)] {
            let inner = preview_inner(Rect::new(0, 0, w, h), PaneId::Left);
            assert!(inner.width <= w && inner.height <= h, "{w}x{h}: {inner:?}");
        }
    }
}
