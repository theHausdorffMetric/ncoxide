use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::error::Result;
use crate::finder::{Finder, FinderMatch};
use crate::mode::normal::NormalState;
use crate::mode::{Action, InputKind, Mode};
use crate::pane::operations;
use crate::pane::{PaneId, PaneState};
use crate::platform;
use crate::preview::{self, PreviewState};
use crate::ui;
use crate::ui::dialog::Dialog;

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
    finder: Finder,
    page_size: usize,
}

impl App {
    pub fn new(left_path: PathBuf, right_path: PathBuf) -> Self {
        App {
            left_pane: PaneState::new(PaneId::Left, left_path),
            right_pane: PaneState::new(PaneId::Right, right_path),
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
            finder: Finder::new(),
            page_size: 20,
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
                && let Event::Key(key) = event::read().map_err(crate::error::NcError::Io)? {
                    self.handle_key(key);
                }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // If there's a dialog, handle it first
        if let Some(ref dialog) = self.dialog.clone() {
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

    fn handle_dialog_key(&mut self, key: KeyEvent, dialog: &Dialog) {
        match dialog {
            Dialog::Confirm { .. } => match key.code {
                crossterm::event::KeyCode::Char('y') | crossterm::event::KeyCode::Char('Y') => {
                    self.dialog = None;
                    self.confirm_delete();
                }
                _ => {
                    self.dialog = None;
                }
            },
            Dialog::Error { .. } | Dialog::Info { .. } => {
                self.dialog = None;
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
                    self.preview_state.scroll_down(1);
                    return;
                }
                Action::HalfPageUp => {
                    let half = self.page_size / 2;
                    self.preview_state.scroll_up(half);
                    return;
                }
                Action::HalfPageDown => {
                    let half = self.page_size / 2;
                    self.preview_state.scroll_down(half);
                    return;
                }
                Action::PageUp => {
                    let ps = self.page_size;
                    self.preview_state.scroll_up(ps);
                    return;
                }
                Action::PageDown => {
                    let ps = self.page_size;
                    self.preview_state.scroll_down(ps);
                    return;
                }
                Action::CursorTop => {
                    self.preview_state.scroll = 0;
                    return;
                }
                Action::CursorBottom => {
                    self.preview_state.scroll = self.preview_state.total_lines.saturating_sub(1);
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
                    && let Some(entry) = self.active_pane_state().current_entry() {
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
                    && !entry.is_dir {
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
            Action::GotoBookmark(_) => {} // TODO: bookmark support

            // Commands
            Action::ExecuteCommand(ref cmd) => self.execute_command_str(cmd),

            // Settings
            Action::ToggleHidden => {
                let pane = self.active_pane_mut();
                pane.show_hidden = !pane.show_hidden;
                let _ = pane.refresh();
            }
            Action::SetSort(sort_by) => {
                let pane = self.active_pane_mut();
                pane.sort_by = sort_by;
                let _ = pane.refresh();
            }
            Action::SetFilter(ref filter) => {
                let pane = self.active_pane_mut();
                pane.filter = filter.clone();
                let _ = pane.refresh();
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
                    && !entry.is_dir {
                        let _ = crate::viewer::view_file(&entry.path);
                    }
            }
        }

        // Auto-return to Normal after single-key modes
        match self.mode {
            Mode::Space | Mode::Goto => {
                if !matches!(action, Action::None | Action::EnterInput(_) | Action::EnterFinder) {
                    self.mode = Mode::Normal;
                }
            }
            _ => {}
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
                let filter = if buf.is_empty() { None } else { Some(buf) };
                let pane = self.active_pane_mut();
                pane.filter = filter;
                let _ = pane.refresh();
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

    fn do_copy(&mut self) {
        let target = self.other_pane_cwd();
        let paths = self.active_pane_state().selected_paths();
        let mut errors = Vec::new();

        for path in &paths {
            if let Err(e) = operations::copy_to(path, &target) {
                errors.push(format!("{}: {e}", path.display()));
            }
        }

        // Refresh both panes
        let _ = self.left_pane.refresh();
        let _ = self.right_pane.refresh();

        if !errors.is_empty() {
            self.dialog = Some(Dialog::Error {
                message: errors.join("\n"),
            });
        }
    }

    fn do_move(&mut self) {
        let target = self.other_pane_cwd();
        let paths = self.active_pane_state().selected_paths();
        let mut errors = Vec::new();

        for path in &paths {
            if let Err(e) = operations::move_to(path, &target) {
                errors.push(format!("{}: {e}", path.display()));
            }
        }

        let _ = self.left_pane.refresh();
        let _ = self.right_pane.refresh();

        if !errors.is_empty() {
            self.dialog = Some(Dialog::Error {
                message: errors.join("\n"),
            });
        }
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
        });
    }

    fn confirm_delete(&mut self) {
        let paths = self.active_pane_state().selected_paths();
        let mut errors = Vec::new();

        for path in &paths {
            if let Err(e) = operations::delete(path) {
                errors.push(format!("{}: {e}", path.display()));
            }
        }

        let _ = self.left_pane.refresh();
        let _ = self.right_pane.refresh();

        if !errors.is_empty() {
            self.dialog = Some(Dialog::Error {
                message: errors.join("\n"),
            });
        }
    }

    fn do_rename(&mut self, new_name: &str) {
        if new_name.is_empty() {
            return;
        }
        if let Some(entry) = self.active_pane_state().current_entry().cloned() {
            if let Err(e) = operations::rename(&entry.path, new_name) {
                self.dialog = Some(Dialog::Error {
                    message: format!("Rename failed: {e}"),
                });
            } else {
                let _ = self.active_pane_mut().refresh();
            }
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
            let _ = self.active_pane_mut().refresh();
        }
    }

    fn edit_file(&mut self) {
        if let Some(entry) = self.active_pane_state().current_entry().cloned() {
            if entry.is_dir {
                return;
            }
            let editor = platform::get_default_editor();
            // Temporarily exit raw mode for the editor
            disable_raw_mode().ok();
            execute!(io::stdout(), LeaveAlternateScreen).ok();

            let _ = Command::new(&editor).arg(&entry.path).status();

            execute!(io::stdout(), EnterAlternateScreen).ok();
            enable_raw_mode().ok();
            let _ = self.active_pane_mut().refresh();
        }
    }

    fn open_file(&mut self, path: &PathBuf) {
        // For now, open with $EDITOR
        let editor = platform::get_default_editor();
        disable_raw_mode().ok();
        execute!(io::stdout(), LeaveAlternateScreen).ok();

        let _ = Command::new(&editor).arg(path).status();

        execute!(io::stdout(), EnterAlternateScreen).ok();
        enable_raw_mode().ok();
        let _ = self.active_pane_mut().refresh();
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
                            && let Some(pos) = self.active_pane_state().entries.iter().position(|e| e.name == name) {
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
            crossterm::event::KeyCode::Char(c) => {
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
            self.preview_state.clear();
            self.preview_state.path = Some(entry.path);
            self.preview_state.lines.push(preview::PreviewLine {
                spans: vec![(
                    "[Directory]".to_string(),
                    ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
                )],
            });
            self.preview_state.total_lines = 1;
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
