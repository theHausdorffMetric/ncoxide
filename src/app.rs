use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::config::Config;
use crate::error::Result;
use crate::finder::{Finder, FinderMatch};
use crate::mode::normal::NormalState;
use crate::mode::{Action, InputKind, Mode};
use crate::pane::operations;
use crate::pane::{PaneId, PaneState, SortBy, SortDirection};
use crate::platform;
use crate::preview::{self, PreviewState};
use crate::ui;
use crate::ui::dialog::{ConfirmAction, Dialog};

/// Application state. Owns both panes and current mode.
pub struct App {
    pub left_pane: PaneState,
    pub right_pane: PaneState,
    pub active_pane: PaneId,
    pub mode: Mode,
    pub input_buffer: String,
    pub input_kind: Option<InputKind>,
    pub show_help: bool,
    pub dialog: Option<Dialog>,
    pub should_quit: bool,
    pub normal_state: NormalState,
    pub finder_results: Vec<FinderMatch>,
    pub finder_cursor: usize,
    pub preview_active: bool,
    pub preview_focused: bool,
    pub preview_state: PreviewState,
    pub config: Config,
    finder: Finder,
    page_size: usize,
    /// A request to hand the terminal to an external program (pager or editor).
    /// Run by the event loop, which owns the `Terminal` and can restore/redraw
    /// the app afterward.
    pending_external: Option<PendingExternal>,
}

/// A full-screen takeover the event loop should run between frames.
enum PendingExternal {
    /// Open the built-in pager on this path.
    View(PathBuf),
    /// Launch `$EDITOR` on this path.
    Edit(PathBuf),
}

impl App {
    /// Construct with default configuration (used by tests and as a fallback).
    pub fn new(left_path: PathBuf, right_path: PathBuf) -> Self {
        Self::new_with_config(left_path, right_path, Config::default())
    }

    /// Construct, seeding both panes from the loaded configuration.
    pub fn new_with_config(left_path: PathBuf, right_path: PathBuf, config: Config) -> Self {
        let mut left_pane = PaneState::new(PaneId::Left, left_path);
        let mut right_pane = PaneState::new(PaneId::Right, right_path);
        Self::apply_general(&mut left_pane, &config);
        Self::apply_general(&mut right_pane, &config);

        App {
            left_pane,
            right_pane,
            active_pane: PaneId::Left,
            mode: Mode::Normal,
            input_buffer: String::new(),
            input_kind: None,
            show_help: false,
            dialog: None,
            should_quit: false,
            normal_state: NormalState::default(),
            finder_results: Vec::new(),
            finder_cursor: 0,
            preview_active: false,
            preview_focused: false,
            preview_state: PreviewState::default(),
            config,
            finder: Finder::new(),
            page_size: 20,
            pending_external: None,
        }
    }

    /// Seed a pane's view settings from `[general]` config, then re-read.
    fn apply_general(pane: &mut PaneState, config: &Config) {
        pane.show_hidden = config.general.show_hidden;
        if let Some(sort_by) = SortBy::parse(&config.general.sort_by) {
            pane.sort_by = sort_by;
        }
        pane.sort_dir = if config.general.sort_ascending {
            SortDirection::Ascending
        } else {
            SortDirection::Descending
        };
        let _ = pane.refresh();
    }

    /// Navigate the active pane to bookmark `index` (0-based). Unset bookmarks
    /// are ignored; a bookmark that no longer points at a directory surfaces
    /// an error dialog.
    fn goto_bookmark(&mut self, index: usize) {
        let Some(path) = self.config.bookmarks.get(index).cloned() else {
            return;
        };
        if path.is_dir() {
            self.active_pane_mut().goto(path);
        } else {
            self.dialog = Some(Dialog::Error {
                message: format!(
                    "Bookmark {} is not an accessible directory: {}",
                    index + 1,
                    path.display()
                ),
            });
        }
    }

    pub fn active_pane_state(&self) -> &PaneState {
        match self.active_pane {
            PaneId::Left => &self.left_pane,
            PaneId::Right => &self.right_pane,
        }
    }

    fn active_pane_mut(&mut self) -> &mut PaneState {
        match self.active_pane {
            PaneId::Left => &mut self.left_pane,
            PaneId::Right => &mut self.right_pane,
        }
    }

    fn other_pane(&self) -> &PaneState {
        match self.active_pane {
            PaneId::Left => &self.right_pane,
            PaneId::Right => &self.left_pane,
        }
    }

    fn other_pane_cwd(&self) -> PathBuf {
        self.other_pane().cwd.clone()
    }

    /// Main event loop.
    pub fn run(&mut self) -> Result<()> {
        // Restore the terminal before the default panic report: the cleanup
        // below only runs on ordinary returns, so without this a panic
        // mid-draw leaves the shell in raw mode on the alternate screen.
        // Also covers the pager, which manages the same terminal state.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            default_hook(info);
        }));

        enable_raw_mode().map_err(crate::error::NcError::Io)?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).map_err(crate::error::NcError::Io)?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).map_err(crate::error::NcError::Io)?;
        terminal.clear().map_err(crate::error::NcError::Io)?;

        let result = self.event_loop(&mut terminal);

        // Cleanup
        disable_raw_mode().ok();
        execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
        terminal.show_cursor().ok();

        result
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            // Update page size from terminal height
            let area = terminal.size().map_err(crate::error::NcError::Io)?;
            self.page_size = area.height.saturating_sub(4) as usize; // borders + header + status

            terminal
                .draw(|f| ui::draw(f, self))
                .map_err(crate::error::NcError::Io)?;

            if self.should_quit {
                return Ok(());
            }

            // Poll for events (50ms timeout for responsive UI)
            if event::poll(Duration::from_millis(50)).map_err(crate::error::NcError::Io)?
                && let Event::Key(key) = event::read().map_err(crate::error::NcError::Io)?
                && key.kind == crossterm::event::KeyEventKind::Press
            {
                self.handle_key(key);
            }

            // Hand the terminal to an external program (pager/editor) if
            // requested, then re-establish the app's TUI and force a full
            // repaint — ratatui's buffer is otherwise out of sync with the
            // now-blank screen, leaving stale borders/rows.
            if let Some(pending) = self.pending_external.take() {
                self.run_external(terminal, pending)?;
            }
        }
    }

    /// Suspend the TUI, run an external full-screen program, then restore the
    /// alternate-screen raw-mode UI and repaint from scratch.
    fn run_external(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        pending: PendingExternal,
    ) -> Result<()> {
        match pending {
            // The pager manages its own raw mode / alternate screen.
            PendingExternal::View(path) => {
                let _ = crate::viewer::view_file(&path);
            }
            // The editor needs cooked mode on the primary screen.
            PendingExternal::Edit(path) => {
                disable_raw_mode().ok();
                execute!(io::stdout(), LeaveAlternateScreen).ok();
                if let Some((program, args)) = self.editor_invocation() {
                    let _ = Command::new(program).args(args).arg(&path).status();
                }
            }
        }

        enable_raw_mode().map_err(crate::error::NcError::Io)?;
        execute!(io::stdout(), EnterAlternateScreen).map_err(crate::error::NcError::Io)?;
        terminal.clear().map_err(crate::error::NcError::Io)?;
        // The file may have changed (editor) and the listing should be current.
        self.refresh_pane(self.active_pane);
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // If there's a dialog, handle it first. Take it out so handlers can
        // freely mutate `self` (and set a follow-up dialog) without cloning.
        if let Some(dialog) = self.dialog.take() {
            self.handle_dialog_key(key, dialog);
            return;
        }

        // If help is showing, any key dismisses it
        if self.show_help {
            if key.code == crossterm::event::KeyCode::Esc
                || key.code == crossterm::event::KeyCode::Char('?')
            {
                self.show_help = false;
            }
            return;
        }

        let action = match self.mode {
            Mode::Normal => crate::mode::normal::handle_key(key, &mut self.normal_state),
            Mode::Select => crate::mode::select::handle_key(key),
            Mode::Space => crate::mode::space::handle_key(key),
            Mode::Goto => crate::mode::goto::handle_key(key),
            Mode::Command => crate::mode::command::handle_key(key),
            Mode::Input(_) => crate::mode::input::handle_key(key),
            Mode::Finder => self.handle_finder_key(key),
        };

        self.dispatch_action(action);
    }

    fn handle_dialog_key(&mut self, key: KeyEvent, dialog: Dialog) {
        // The dialog was already taken out of `self` by the caller, so any
        // unhandled key simply dismisses it.
        if let Dialog::Confirm { action, .. } = dialog
            && matches!(
                key.code,
                crossterm::event::KeyCode::Char('y') | crossterm::event::KeyCode::Char('Y')
            )
        {
            self.execute_confirm(action);
        }
    }

    fn execute_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Delete { paths } => self.execute_delete(&paths),
            ConfirmAction::OverwriteCopy { sources, target } => {
                self.execute_copy(&sources, &target);
            }
            ConfirmAction::OverwriteMove { sources, target } => {
                self.execute_move(&sources, &target);
            }
            ConfirmAction::OverwriteRename { source, new_name } => {
                self.execute_rename(&source, &new_name);
            }
        }
    }

    fn dispatch_action(&mut self, action: Action) {
        // When preview is focused, intercept navigation to scroll preview
        if self.preview_active && self.preview_focused {
            match action {
                Action::CursorUp => {
                    self.preview_state.scroll_up(1);
                    return;
                }
                Action::CursorDown => {
                    self.preview_state.scroll_down(1, self.page_size);
                    return;
                }
                Action::HalfPageUp => {
                    let half = self.page_size / 2;
                    self.preview_state.scroll_up(half);
                    return;
                }
                Action::HalfPageDown => {
                    let half = self.page_size / 2;
                    self.preview_state.scroll_down(half, self.page_size);
                    return;
                }
                Action::PageUp => {
                    let ps = self.page_size;
                    self.preview_state.scroll_up(ps);
                    return;
                }
                Action::PageDown => {
                    let ps = self.page_size;
                    self.preview_state.scroll_down(ps, self.page_size);
                    return;
                }
                Action::CursorTop => {
                    self.preview_state.scroll_to_top();
                    return;
                }
                Action::CursorBottom => {
                    self.preview_state.scroll_to_bottom(self.page_size);
                    return;
                }
                Action::ExitToNormal => {
                    self.preview_focused = false;
                    self.mode = Mode::Normal;
                    self.normal_state = NormalState::default();
                    return;
                }
                // SwitchPane / FocusLeft / FocusRight toggle focus (handled below)
                Action::SwitchPane | Action::FocusLeftPane | Action::FocusRightPane => {}
                // Quit and TogglePreview pass through
                Action::Quit | Action::TogglePreview => {}
                // Everything else is a no-op when preview is focused
                _ => return,
            }
        }

        match action {
            Action::None => {}
            Action::Quit => self.should_quit = true,
            Action::Redraw => {}

            // Navigation
            Action::CursorUp => self.active_pane_mut().cursor_up(),
            Action::CursorDown => self.active_pane_mut().cursor_down(),
            Action::CursorTop => self.active_pane_mut().cursor_top(),
            Action::CursorBottom => self.active_pane_mut().cursor_bottom(),
            Action::PageUp => {
                let ps = self.page_size;
                self.active_pane_mut().page_up(ps);
            }
            Action::PageDown => {
                let ps = self.page_size;
                self.active_pane_mut().page_down(ps);
            }
            Action::HalfPageUp => {
                let ps = self.page_size;
                self.active_pane_mut().half_page_up(ps);
            }
            Action::HalfPageDown => {
                let ps = self.page_size;
                self.active_pane_mut().half_page_down(ps);
            }
            Action::EnterDir => {
                if let Some(file_path) = self.active_pane_mut().enter() {
                    self.open_file(&file_path);
                }
            }
            Action::ParentDir => self.active_pane_mut().go_parent(),
            Action::SwitchPane | Action::FocusLeftPane | Action::FocusRightPane => {
                if self.preview_active {
                    self.preview_focused = !self.preview_focused;
                } else {
                    match action {
                        Action::SwitchPane => self.active_pane = self.active_pane.other(),
                        Action::FocusLeftPane => self.active_pane = PaneId::Left,
                        Action::FocusRightPane => self.active_pane = PaneId::Right,
                        _ => unreachable!(),
                    }
                }
            }

            // Mode transitions
            Action::EnterSelect => self.mode = Mode::Select,
            Action::EnterSpace => self.mode = Mode::Space,
            Action::EnterGoto => self.mode = Mode::Goto,
            Action::EnterCommand => {
                self.mode = Mode::Command;
                self.input_buffer.clear();
            }
            Action::EnterInput(kind) => {
                self.mode = Mode::Input(kind);
                self.input_kind = Some(kind);
                self.input_buffer.clear();
                // Pre-fill rename with current filename
                if kind == InputKind::Rename
                    && let Some(entry) = self.active_pane_state().current_entry()
                {
                    self.input_buffer = entry.name.clone();
                }
            }
            Action::ExitToNormal => {
                self.mode = Mode::Normal;
                self.normal_state = NormalState::default();
            }

            // Selection
            Action::ToggleSelect => self.active_pane_mut().toggle_select(),
            Action::SelectAll => self.active_pane_mut().select_all(),
            Action::InvertSelection => self.active_pane_mut().invert_selection(),
            Action::SelectExtendDown => self.active_pane_mut().select_extend_down(),
            Action::SelectExtendUp => self.active_pane_mut().select_extend_up(),

            // File operations
            Action::CopyToOther => self.do_copy(),
            Action::MoveToOther => self.do_move(),
            Action::DeleteSelected => self.confirm_delete_dialog(),
            Action::Rename(ref new_name) => self.do_rename(new_name),
            Action::Mkdir(ref name) => self.do_mkdir(name),
            Action::EditFile => self.edit_file(),
            Action::OpenFile => {
                if let Some(entry) = self.active_pane_state().current_entry().cloned()
                    && !entry.is_dir
                {
                    self.open_file(&entry.path);
                }
            }

            // Goto
            Action::GotoHome => {
                if let Some(home) = dirs::home_dir() {
                    self.active_pane_mut().goto(home);
                }
            }
            Action::GotoRoot => {
                self.active_pane_mut().goto(PathBuf::from("/"));
            }
            Action::GotoOtherPane => {
                let other_cwd = self.other_pane_cwd();
                self.active_pane_mut().goto(other_cwd);
            }
            Action::GotoPrevious => {
                let prev = self.active_pane_state().prev_dir.clone();
                if let Some(prev) = prev {
                    self.active_pane_mut().goto(prev);
                }
            }
            Action::GotoBookmark(index) => self.goto_bookmark(index),

            // Commands
            Action::ExecuteCommand(ref cmd) => self.execute_command_str(cmd),

            // Settings
            Action::ToggleHidden => {
                let id = self.active_pane;
                let pane = self.active_pane_mut();
                pane.show_hidden = !pane.show_hidden;
                self.refresh_pane(id);
            }
            Action::SetSort(sort_by) => {
                let id = self.active_pane;
                self.active_pane_mut().sort_by = sort_by;
                self.refresh_pane(id);
            }
            Action::SetFilter(ref filter) => {
                let id = self.active_pane;
                self.active_pane_mut().filter = filter.clone();
                self.refresh_pane(id);
            }

            // Info
            Action::ShowHelp => self.show_help = !self.show_help,
            Action::ShowFileInfo => self.show_file_info(),

            // Input handling
            Action::InputChar(c) => self.input_buffer.push(c),
            Action::InputBackspace => {
                self.input_buffer.pop();
            }
            Action::InputConfirm => self.handle_input_confirm(),
            Action::InputCancel => {
                self.input_buffer.clear();
                self.input_kind = None;
                self.mode = Mode::Normal;
            }

            // Finder mode
            Action::EnterFinder => self.enter_finder(),
            Action::FinderSelect(_) | Action::FinderCursorUp | Action::FinderCursorDown => {
                // Handled directly in handle_finder_key
            }

            // Preview mode
            Action::TogglePreview => self.toggle_preview(),
            Action::ViewFile => {
                if let Some(entry) = self.active_pane_state().current_entry().cloned()
                    && !entry.is_dir
                {
                    // Defer to the event loop, which owns the Terminal and
                    // can fully restore/redraw the app after the viewer exits.
                    self.pending_external = Some(PendingExternal::View(entry.path));
                }
            }
        }

        // Auto-return to Normal after single-key modes
        if matches!(self.mode, Mode::Space | Mode::Goto)
            && !matches!(
                action,
                Action::None
                    | Action::EnterSpace
                    | Action::EnterGoto
                    | Action::EnterInput(_)
                    | Action::EnterFinder
            )
        {
            self.mode = Mode::Normal;
        }

        // Update preview when cursor changes
        self.update_preview();
    }

    fn handle_input_confirm(&mut self) {
        let buf = self.input_buffer.clone();
        let kind = self.input_kind;
        self.input_buffer.clear();
        self.input_kind = None;
        self.mode = Mode::Normal;

        match kind {
            Some(InputKind::Rename) => self.do_rename(&buf),
            Some(InputKind::Mkdir) => self.do_mkdir(&buf),
            Some(InputKind::Search) => {
                let id = self.active_pane;
                self.active_pane_mut().filter = if buf.is_empty() { None } else { Some(buf) };
                self.refresh_pane(id);
            }
            Some(InputKind::GlobSelect) => {
                self.active_pane_mut().select_by_glob(&buf);
            }
            Some(InputKind::CommandLine) | None => {
                let action = crate::mode::command::execute_command(&buf);
                self.dispatch_action(action);
            }
        }
    }

    /// Refresh a single pane, surfacing any failure (e.g. the cwd was
    /// unmounted or its permissions revoked) as an error dialog instead of
    /// leaving a stale listing on screen.
    fn refresh_pane(&mut self, id: PaneId) {
        let result = match id {
            PaneId::Left => self.left_pane.refresh(),
            PaneId::Right => self.right_pane.refresh(),
        };
        if let Err(e) = result {
            self.dialog = Some(Dialog::Error {
                message: format!("Failed to refresh directory: {e}"),
            });
        }
    }

    fn refresh_both(&mut self) {
        self.refresh_pane(PaneId::Left);
        self.refresh_pane(PaneId::Right);
    }

    /// Build an overwrite-confirmation message for the names in `paths` that
    /// already exist in `target`, or `None` if there are no collisions.
    fn collision_message(paths: &[PathBuf], target: &Path) -> Option<String> {
        let collisions: Vec<_> = paths
            .iter()
            .filter_map(|p| p.file_name())
            .filter(|name| target.join(name).symlink_metadata().is_ok())
            .map(|name| name.to_string_lossy().to_string())
            .collect();
        match collisions.as_slice() {
            [] => None,
            [one] => Some(format!("Overwrite {one}?")),
            many => Some(format!("Overwrite {} items?", many.len())),
        }
    }

    /// Run `op` over every path, collect per-path failures, refresh both
    /// panes, and surface any failures as an error dialog.
    fn run_batch(&mut self, paths: &[PathBuf], mut op: impl FnMut(&Path) -> Result<()>) {
        let mut errors = Vec::new();
        for path in paths {
            if let Err(e) = op(path) {
                errors.push(format!("{}: {e}", path.display()));
            }
        }

        self.refresh_both();

        if !errors.is_empty() {
            self.dialog = Some(Dialog::Error {
                message: errors.join("\n"),
            });
        }
    }

    fn do_copy(&mut self) {
        let target = self.other_pane_cwd();
        let paths = self.active_pane_state().selected_paths();
        if paths.is_empty() {
            return;
        }

        match Self::collision_message(&paths, &target) {
            None => self.execute_copy(&paths, &target),
            Some(message) => {
                self.dialog = Some(Dialog::Confirm {
                    title: "Confirm Copy".into(),
                    message,
                    action: ConfirmAction::OverwriteCopy {
                        sources: paths,
                        target,
                    },
                })
            }
        }
    }

    fn execute_copy(&mut self, paths: &[PathBuf], target: &Path) {
        if let Some(msg) = self.check_disk_space(paths, target) {
            self.dialog = Some(Dialog::Error { message: msg });
            return;
        }
        self.run_batch(paths, |path| operations::copy_to(path, target));
    }

    fn do_move(&mut self) {
        let target = self.other_pane_cwd();
        let paths = self.active_pane_state().selected_paths();
        if paths.is_empty() {
            return;
        }

        match Self::collision_message(&paths, &target) {
            None => self.execute_move(&paths, &target),
            Some(message) => {
                self.dialog = Some(Dialog::Confirm {
                    title: "Confirm Move".into(),
                    message,
                    action: ConfirmAction::OverwriteMove {
                        sources: paths,
                        target,
                    },
                })
            }
        }
    }

    fn execute_move(&mut self, paths: &[PathBuf], target: &Path) {
        // No space pre-check here: a same-filesystem move is a rename and
        // needs no free space. The cross-device copy+delete fallback checks
        // space inside `move_to` before copying anything.
        self.run_batch(paths, |path| operations::move_to(path, target));
    }

    fn confirm_delete_dialog(&mut self) {
        let paths = self.active_pane_state().selected_paths();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        let message = if count == 1 {
            format!("Delete {}?", paths[0].display())
        } else {
            format!("Delete {count} items?")
        };
        self.dialog = Some(Dialog::Confirm {
            title: "Confirm Delete".into(),
            message,
            action: ConfirmAction::Delete { paths },
        });
    }

    fn execute_delete(&mut self, paths: &[PathBuf]) {
        self.run_batch(paths, operations::delete);
    }

    fn do_rename(&mut self, new_name: &str) {
        if new_name.is_empty() {
            return;
        }
        let Some(entry) = self.active_pane_state().current_entry().cloned() else {
            return;
        };
        let Some(parent) = entry.path.parent() else {
            return;
        };

        if parent.join(new_name).symlink_metadata().is_ok() {
            self.dialog = Some(Dialog::Confirm {
                title: "Confirm Rename".into(),
                message: format!("Overwrite {new_name}?"),
                action: ConfirmAction::OverwriteRename {
                    source: entry.path.clone(),
                    new_name: new_name.to_string(),
                },
            });
        } else {
            self.execute_rename(&entry.path, new_name);
        }
    }

    fn execute_rename(&mut self, source: &Path, new_name: &str) {
        if let Err(e) = operations::rename(source, new_name) {
            self.dialog = Some(Dialog::Error {
                message: format!("Rename failed: {e}"),
            });
        } else {
            self.refresh_pane(self.active_pane);
        }
    }

    /// Check if there's enough disk space for a copy (moves rename in place
    /// and only need space in the cross-device fallback, checked in
    /// `operations::move_to`).
    /// Returns Some(error message) if space is insufficient, None if OK or undetermined.
    fn check_disk_space(&self, sources: &[PathBuf], target: &Path) -> Option<String> {
        let free = platform::get_free_disk_space(target)?;
        let mut total_size = 0u64;
        for source in sources {
            // If path_size fails, skip (fail-open)
            if let Ok(size) = operations::path_size(source) {
                total_size = total_size.saturating_add(size);
            }
        }
        if total_size > free {
            Some(format!(
                "Not enough disk space. Need {} but only {} available.",
                platform::format_file_size(total_size),
                platform::format_file_size(free),
            ))
        } else {
            None
        }
    }

    fn do_mkdir(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        let cwd = self.active_pane_state().cwd.clone();
        if let Err(e) = operations::mkdir(&cwd, name) {
            self.dialog = Some(Dialog::Error {
                message: format!("Mkdir failed: {e}"),
            });
        } else {
            self.refresh_pane(self.active_pane);
        }
    }

    /// Editor invocation: the configured `[general] editor` if set, otherwise
    /// `$EDITOR` (falling back to `vi`). Split on whitespace into program +
    /// args so values like `code -w` or `emacsclient -t` work; quoting is not
    /// supported (the common convention — same as less, crontab, etc.).
    /// `None` for a blank editor value.
    fn editor_invocation(&self) -> Option<(String, Vec<String>)> {
        let raw = self
            .config
            .general
            .editor
            .clone()
            .unwrap_or_else(platform::get_default_editor);
        let mut parts = raw.split_whitespace().map(str::to_string);
        let program = parts.next()?;
        Some((program, parts.collect()))
    }

    fn edit_file(&mut self) {
        if let Some(entry) = self.active_pane_state().current_entry().cloned()
            && !entry.is_dir
        {
            // Defer to the event loop (it owns the Terminal to restore/redraw).
            self.pending_external = Some(PendingExternal::Edit(entry.path));
        }
    }

    fn open_file(&mut self, path: &Path) {
        self.pending_external = Some(PendingExternal::Edit(path.to_path_buf()));
    }

    fn show_file_info(&mut self) {
        if let Some(entry) = self.active_pane_state().current_entry() {
            let info = format!(
                "Name: {}\nPath: {}\nSize: {}\nPermissions: {}\nModified: {}",
                entry.name,
                entry.path.display(),
                platform::format_file_size(entry.size),
                entry.permissions,
                entry
                    .modified
                    .map(platform::format_file_time)
                    .unwrap_or_else(|| "unknown".into()),
            );
            self.dialog = Some(Dialog::Info {
                title: "File Info".into(),
                message: info,
            });
        }
    }

    fn execute_command_str(&mut self, cmd: &str) {
        if let Some(path_str) = cmd.strip_prefix("cd ") {
            let path = PathBuf::from(path_str.trim());
            let path = if path.is_absolute() {
                path
            } else {
                self.active_pane_state().cwd.join(path)
            };
            self.active_pane_mut().goto(path);
        }
    }

    fn handle_finder_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            crossterm::event::KeyCode::Esc => {
                self.finder_results.clear();
                self.finder_cursor = 0;
                Action::ExitToNormal
            }
            crossterm::event::KeyCode::Enter => {
                if let Some(result) = self.finder_results.get(self.finder_cursor) {
                    let path = result.path.clone();
                    self.finder_results.clear();
                    self.finder_cursor = 0;
                    self.mode = Mode::Normal;
                    if path.is_dir() {
                        self.active_pane_mut().goto(path);
                    } else if let Some(parent) = path.parent() {
                        let parent = parent.to_path_buf();
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
                        self.active_pane_mut().goto(parent);
                        // Try to position cursor on the file
                        if let Some(name) = name
                            && let Some(pos) = self
                                .active_pane_state()
                                .entries
                                .iter()
                                .position(|e| e.name == name)
                        {
                            self.active_pane_mut().cursor = pos;
                        }
                    }
                }
                Action::None
            }
            crossterm::event::KeyCode::Up => {
                if self.finder_cursor > 0 {
                    self.finder_cursor -= 1;
                }
                Action::None
            }
            crossterm::event::KeyCode::Down => {
                if !self.finder_results.is_empty()
                    && self.finder_cursor < self.finder_results.len() - 1
                {
                    self.finder_cursor += 1;
                }
                Action::None
            }
            crossterm::event::KeyCode::Backspace => {
                self.input_buffer.pop();
                self.update_finder_results();
                Action::None
            }
            crossterm::event::KeyCode::Char(c) if crate::mode::accepts_text(&key) => {
                self.input_buffer.push(c);
                self.update_finder_results();
                Action::None
            }
            _ => Action::None,
        }
    }

    fn toggle_preview(&mut self) {
        self.preview_active = !self.preview_active;
        if self.preview_active {
            self.update_preview();
        } else {
            self.preview_focused = false;
            self.preview_state.clear();
        }
    }

    fn update_preview(&mut self) {
        if !self.preview_active {
            return;
        }
        let Some(entry) = self.active_pane_state().current_entry().cloned() else {
            return;
        };
        // Only update if path changed
        if self.preview_state.path.as_ref() == Some(&entry.path) {
            return;
        }
        if entry.is_dir {
            self.preview_state = PreviewState::message(
                Some(entry.path),
                "[Directory]",
                ratatui::style::Color::DarkGray,
            );
        } else {
            self.preview_state = preview::load_preview(&entry.path);
        }
    }

    fn enter_finder(&mut self) {
        self.mode = Mode::Finder;
        self.input_buffer.clear();
        self.finder_results.clear();
        self.finder_cursor = 0;
    }

    fn update_finder_results(&mut self) {
        let cwd = self.active_pane_state().cwd.clone();
        self.finder_results = self.finder.find(&cwd, &self.input_buffer, 20);
        self.finder_cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, GeneralConfig};
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("ncoxide_app_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn test_new_with_config_seeds_panes() {
        let dir = temp_dir("seed");
        fs::write(dir.join(".hidden"), "").unwrap();
        fs::write(dir.join("visible.txt"), "").unwrap();

        let config = Config {
            general: GeneralConfig {
                show_hidden: true,
                sort_by: "size".into(),
                sort_ascending: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let app = App::new_with_config(dir.clone(), dir.clone(), config);
        // show_hidden from config is applied, so the hidden file is listed.
        assert!(app.left_pane.show_hidden);
        assert_eq!(app.left_pane.sort_by, SortBy::Size);
        assert_eq!(app.left_pane.sort_dir, SortDirection::Descending);
        assert!(app.left_pane.entries.iter().any(|e| e.name == ".hidden"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_editor_invocation_splits_program_and_args() {
        let dir = temp_dir("editor");

        let mut config = Config::default();
        config.general.editor = Some("code -w --reuse-window".into());
        let app = App::new_with_config(dir.clone(), dir.clone(), config);
        assert_eq!(
            app.editor_invocation(),
            Some((
                "code".to_string(),
                vec!["-w".to_string(), "--reuse-window".to_string()]
            ))
        );

        let mut config = Config::default();
        config.general.editor = Some("vi".into());
        let app = App::new_with_config(dir.clone(), dir.clone(), config);
        assert_eq!(app.editor_invocation(), Some(("vi".to_string(), vec![])));

        // Blank editor value: nothing to run.
        let mut config = Config::default();
        config.general.editor = Some("   ".into());
        let app = App::new_with_config(dir.clone(), dir.clone(), config);
        assert_eq!(app.editor_invocation(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_goto_bookmark_navigates_and_reports_missing() {
        let root = temp_dir("bookmark");
        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();

        let config = Config {
            bookmarks: vec![target.clone(), root.join("does_not_exist")],
            ..Default::default()
        };
        let mut app = App::new_with_config(root.clone(), root.clone(), config);

        // Bookmark 0 -> existing directory: active pane navigates there.
        app.goto_bookmark(0);
        assert_eq!(app.active_pane_state().cwd, target);
        assert!(app.dialog.is_none());

        // Bookmark 1 -> missing path: surfaces an error dialog, no navigation.
        app.goto_bookmark(1);
        assert!(matches!(app.dialog, Some(Dialog::Error { .. })));

        // Out-of-range index is a no-op.
        app.dialog = None;
        app.goto_bookmark(99);
        assert!(app.dialog.is_none());

        let _ = fs::remove_dir_all(&root);
    }
}

/// End-to-end tests: drive key sequences through `handle_key` (the same entry
/// point the event loop uses) and render with `ratatui`'s `TestBackend`, then
/// assert on both `App` state and the rendered frame. No real terminal is
/// touched (we never call `App::run`).
#[cfg(test)]
mod integration {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::fs;

    /// Create a uniquely-named temp dir containing `files` and an `App` rooted
    /// there. `name` must be unique per test so parallel runs don't collide.
    fn app_with(name: &str, files: &[&str]) -> (PathBuf, App) {
        let dir = std::env::temp_dir().join(format!("ncoxide_it_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for f in files {
            fs::write(dir.join(f), b"contents\n").unwrap();
        }
        let app = App::new(dir.clone(), dir.clone());
        (dir, app)
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_key(key(c));
        }
    }

    /// Render the app to a `TestBackend` and flatten the buffer to text.
    fn render(app: &App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let area = *buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn test_copy_to_same_dir_refused_and_file_intact() {
        // Both panes share a cwd (the app's default startup state). `y` must
        // not destroy the file: overwrite prompt -> confirm -> guarded error,
        // content untouched (R1).
        let (dir, mut app) = app_with("selfcopy", &["data.txt"]);
        fs::write(dir.join("data.txt"), b"important").unwrap();

        app.handle_key(key('y'));
        assert!(
            matches!(app.dialog, Some(Dialog::Confirm { .. })),
            "same-name copy should raise the overwrite confirm"
        );
        app.handle_key(key('y'));
        assert!(
            matches!(app.dialog, Some(Dialog::Error { .. })),
            "confirmed self-copy must surface an error, not run"
        );
        assert_eq!(
            fs::read_to_string(dir.join("data.txt")).unwrap(),
            "important"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ctrl_chords_do_not_type_into_inputs() {
        // Ctrl-C during a rename used to insert a literal 'c' (R9).
        let (dir, mut app) = app_with("ctrl_input", &["file.txt"]);
        app.handle_key(key('r')); // rename input, pre-filled with the name
        assert_eq!(app.input_buffer, "file.txt");

        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(app.input_buffer, "file.txt", "Ctrl-C must not type");
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(app.input_buffer, "file.txt", "Alt-x must not type");

        app.handle_key(key('2'));
        assert_eq!(app.input_buffer, "file.txt2", "plain chars still type");

        // Finder input takes the same guard.
        app.handle_key(code(KeyCode::Esc));
        app.mode = Mode::Finder;
        app.input_buffer.clear();
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(app.input_buffer, "", "Ctrl-C must not type in finder");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_move_via_space_menu_between_panes() {
        // Same-filesystem move driven through the real key path (Space, m).
        // Must succeed without any disk-space pre-check getting in the way
        // (R5: moves rename in place and need no free space).
        let (dir, mut app) = app_with("move_sfs", &["moveme.txt"]);
        let dst = dir.join("dst");
        fs::create_dir_all(&dst).unwrap();
        let _ = app.left_pane.refresh();
        app.right_pane.goto(dst.clone());

        // Cursor onto the file ("dst" dir sorts first).
        let idx = app
            .left_pane
            .entries
            .iter()
            .position(|e| e.name == "moveme.txt")
            .unwrap();
        app.left_pane.cursor = idx;

        app.handle_key(key(' '));
        app.handle_key(key('m'));

        assert!(app.dialog.is_none(), "move should not raise a dialog");
        assert!(!dir.join("moveme.txt").exists());
        assert!(dst.join("moveme.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_navigation_moves_cursor_and_renders() {
        let (dir, mut app) = app_with("nav", &["a.txt", "b.txt", "c.txt"]);
        assert_eq!(app.left_pane.cursor, 0);

        app.handle_key(key('j'));
        assert_eq!(app.left_pane.cursor, 1);
        app.handle_key(key('j'));
        assert_eq!(app.left_pane.cursor, 2);
        app.handle_key(key('k'));
        assert_eq!(app.left_pane.cursor, 1);

        let screen = render(&app, 80, 24);
        assert!(screen.contains("a.txt"));
        assert!(screen.contains("c.txt"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_enter_and_parent_directory() {
        let (dir, mut app) = app_with("enter", &[]);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("inner.txt"), b"x").unwrap();
        let _ = app.left_pane.refresh();

        // "sub" sorts first (directories before files).
        app.handle_key(key('l'));
        assert!(app.left_pane.cwd.ends_with("sub"));
        assert!(app.left_pane.entries.iter().any(|e| e.name == "inner.txt"));

        app.handle_key(key('h'));
        assert_eq!(app.left_pane.cwd, dir);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tab_switches_pane() {
        let (dir, mut app) = app_with("tab", &["a.txt"]);
        assert_eq!(app.active_pane, PaneId::Left);
        app.handle_key(code(KeyCode::Tab));
        assert_eq!(app.active_pane, PaneId::Right);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_select_all_shows_count_in_status() {
        let (dir, mut app) = app_with("select", &["a.txt", "b.txt", "c.txt"]);
        app.handle_key(key('v')); // enter Select mode
        app.handle_key(key('a')); // select all
        assert_eq!(app.left_pane.selection_count(), 3);

        // Wide enough that the long temp-dir path doesn't clip the status text.
        let screen = render(&app, 200, 24);
        assert!(
            screen.contains("selected"),
            "status should show selection count"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_confirm_then_accept_removes_file() {
        let (dir, mut app) = app_with("del_accept", &["target.txt", "keep.txt"]);
        // cursor on "keep.txt"? entries sorted: keep.txt, target.txt
        // Put cursor on target.txt.
        let idx = app
            .left_pane
            .entries
            .iter()
            .position(|e| e.name == "target.txt")
            .unwrap();
        app.left_pane.cursor = idx;

        app.handle_key(key('d'));
        assert!(app.dialog.is_some(), "delete should raise a confirm dialog");
        let screen = render(&app, 80, 24);
        assert!(screen.contains("Delete"));
        assert!(screen.contains("[y]es"));

        app.handle_key(key('y'));
        assert!(app.dialog.is_none());
        assert!(!dir.join("target.txt").exists());
        assert!(dir.join("keep.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_dismissed_keeps_file() {
        let (dir, mut app) = app_with("del_dismiss", &["target.txt"]);
        app.handle_key(key('d'));
        assert!(app.dialog.is_some());
        app.handle_key(key('n')); // any non-y dismisses
        assert!(app.dialog.is_none());
        assert!(dir.join("target.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mkdir_via_space_menu() {
        let (dir, mut app) = app_with("mkdir", &["a.txt"]);
        app.handle_key(key(' ')); // space menu
        app.handle_key(key('n')); // new directory -> input mode
        type_str(&mut app, "created");
        app.handle_key(code(KeyCode::Enter));

        assert!(dir.join("created").is_dir());
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.left_pane.entries.iter().any(|e| e.name == "created"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_help_overlay_shows_and_dismisses() {
        let (dir, mut app) = app_with("help", &["a.txt"]);
        app.handle_key(key('?'));
        assert!(app.show_help);
        let screen = render(&app, 80, 24);
        assert!(screen.contains("Navigation"));

        app.handle_key(code(KeyCode::Esc));
        assert!(!app.show_help);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_preview_toggle_renders_preview_pane() {
        let (dir, mut app) = app_with("preview", &["a.txt"]);
        app.handle_key(key('p'));
        assert!(app.preview_active);
        let screen = render(&app, 100, 24);
        assert!(screen.contains("PREVIEW"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_command_mode_prompt_and_escape() {
        let (dir, mut app) = app_with("command", &["a.txt"]);
        app.handle_key(key(':'));
        assert_eq!(app.mode, Mode::Command);
        let screen = render(&app, 80, 24);
        assert!(screen.contains("CMD"));

        app.handle_key(code(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_render_is_panic_free_at_extreme_sizes() {
        let (dir, app) = app_with("empty", &[]); // empty directory
        // Tiny and large terminals, empty listing — must not panic.
        let _ = render(&app, 1, 1);
        let _ = render(&app, 20, 3);
        let _ = render(&app, 200, 60);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_overlays_render_panic_free_at_tiny_sizes() {
        // Overlay geometry used raw u16 subtraction and could underflow on
        // terminals smaller than the overlay margins (R3). Exercise every
        // overlay at pathological sizes.
        let (dir, mut app) = app_with("tiny_overlays", &["a.txt"]);
        for (w, h) in [(1u16, 1u16), (3, 2), (6, 3), (20, 3), (39, 5), (80, 24)] {
            app.show_help = true;
            let _ = render(&app, w, h);
            app.show_help = false;

            app.dialog = Some(Dialog::Error {
                message: "boom".into(),
            });
            let _ = render(&app, w, h);
            app.dialog = None;

            app.mode = Mode::Space;
            let _ = render(&app, w, h);

            app.mode = Mode::Finder;
            app.input_buffer = "q".into();
            let _ = render(&app, w, h);
            app.mode = Mode::Normal;
            app.input_buffer.clear();
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
