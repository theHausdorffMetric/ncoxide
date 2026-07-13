pub mod command;
pub mod goto;
pub mod input;
pub mod normal;
pub mod select;
pub mod space;

use crossterm::event::{KeyEvent, KeyModifiers};

/// True when a `KeyCode::Char` event is plain typed text: no modifiers beyond
/// SHIFT (which legitimately produces uppercase/symbols). Without this guard,
/// chords like Ctrl-C insert their letter into text inputs.
pub fn accepts_text(key: &KeyEvent) -> bool {
    key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
}

/// Application mode — determines how keystrokes are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Select,
    Space,
    Goto,
    Command,
    Input(InputKind),
    Finder,
}

/// What kind of text input we're collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Rename,
    Mkdir,
    Search,
    GlobSelect,
    CommandLine,
}

impl Mode {
    /// Short label for the status line.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NOR",
            Mode::Select => "SEL",
            Mode::Space => "SPC",
            Mode::Goto => "GTO",
            Mode::Command => "CMD",
            Mode::Input(_) => "INP",
            Mode::Finder => "FND",
        }
    }
}

/// Actions returned by mode handlers, dispatched by the app event loop.
#[derive(Debug, Clone)]
pub enum Action {
    None,
    Quit,
    Redraw,

    // Navigation
    CursorUp,
    CursorDown,
    CursorTop,
    CursorBottom,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    EnterDir,
    ParentDir,
    SwitchPane,
    FocusLeftPane,
    FocusRightPane,

    // Mode transitions
    EnterSelect,
    EnterSpace,
    EnterGoto,
    EnterCommand,
    EnterInput(InputKind),
    ExitToNormal,

    // Select mode
    ToggleSelect,
    SelectAll,
    InvertSelection,
    SelectExtendDown,
    SelectExtendUp,
    /// Clear the selection (Helix-style `;` collapse; selections otherwise
    /// persist across mode changes, so clearing is an explicit action).
    DeselectAll,

    // File operations
    CopyToOther,
    MoveToOther,
    DeleteSelected,
    Rename(String),
    Mkdir(String),
    EditFile,
    OpenFile,

    // Goto
    GotoHome,
    GotoRoot,
    GotoOtherPane,
    GotoPrevious,
    GotoBookmark(usize),

    // Command mode
    ExecuteCommand(String),

    // Settings
    ToggleHidden,
    SetSort(crate::pane::SortBy),
    SetFilter(Option<String>),

    // Info
    ShowHelp,
    ShowFileInfo,

    // Input mode
    InputChar(char),
    InputBackspace,
    InputConfirm,
    InputCancel,

    // Finder mode
    EnterFinder,
    FinderSelect(usize),
    FinderCursorUp,
    FinderCursorDown,

    // Preview mode
    TogglePreview,
    ViewFile,
}
