//! Every command in one registry, so the classic toolbars, the ribbon and the menus all show
//! the same actions with the same icons, labels and shortcuts.

use crate::icons::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Save,
    Undo,
    Redo,
    Select,
    PlaceSymbol,
    DrawWire,
    Delete,
    Rotate,
    AddPage,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ToggleGrid,
    LibraryStatus,
    LibraryUpdate,
    ExportPdf,
    Commit,
    NewBranch,
    History,
    ToggleChrome,
}

pub struct Info {
    pub label: &'static str,
    pub tip: &'static str,
    pub icon: Icon,
    pub shortcut: &'static str,
}

impl Action {
    pub fn info(self) -> Info {
        let i = |label, tip, icon, shortcut| Info {
            label,
            tip,
            icon,
            shortcut,
        };
        match self {
            Action::Open => i("Open", "Open a project folder", Icon::Open, "Ctrl+O"),
            Action::Save => i("Save", "Save all changed pages", Icon::Save, "Ctrl+S"),
            Action::Undo => i("Undo", "Undo the last change", Icon::Undo, "Ctrl+Z"),
            Action::Redo => i("Redo", "Redo", Icon::Redo, "Ctrl+Y"),
            Action::Select => i("Select", "Select and move", Icon::Select, "Esc"),
            Action::PlaceSymbol => i(
                "Symbol",
                "Place the symbol chosen in the library picker",
                Icon::Symbol,
                "S",
            ),
            Action::DrawWire => i(
                "Wire",
                "Draw a wire: click a pin or point, then the other end",
                Icon::Wire,
                "W",
            ),
            Action::Delete => i("Delete", "Delete the selection", Icon::Delete, "Del"),
            Action::Rotate => i("Rotate", "Rotate the selected symbol 90°", Icon::Rotate, "R"),
            Action::AddPage => i("New page", "Add a schematic page", Icon::PageAdd, "Ctrl+N"),
            Action::ZoomIn => i("Zoom in", "Zoom in", Icon::ZoomIn, "+"),
            Action::ZoomOut => i("Zoom out", "Zoom out", Icon::ZoomOut, "-"),
            Action::ZoomFit => i("Fit page", "Show the whole page", Icon::ZoomFit, "F"),
            Action::ToggleGrid => i("Grid", "Show or hide the grid", Icon::Grid, "G"),
            Action::LibraryStatus => i(
                "Library",
                "Compare the project's library items with the library",
                Icon::Library,
                "",
            ),
            Action::LibraryUpdate => i("Update", "Take the newer library versions", Icon::Update, ""),
            Action::ExportPdf => i("PDF", "Export all pages to PDF", Icon::Pdf, "Ctrl+P"),
            Action::Commit => i("Commit", "Record a version of the project", Icon::Commit, "Ctrl+K"),
            Action::NewBranch => i("Branch", "Start a branch from this version", Icon::Branch, ""),
            Action::History => i("History", "Show the version history as a subway map", Icon::History, ""),
            Action::ToggleChrome => i(
                "Layout",
                "Switch between classic toolbars and the ribbon",
                Icon::Layout,
                "",
            ),
        }
    }
}

/// Classic toolbars: groups separated by a divider.
pub const TOOLBAR: &[&[Action]] = &[
    &[Action::Open, Action::Save],
    &[Action::Undo, Action::Redo],
    &[
        Action::Select,
        Action::PlaceSymbol,
        Action::DrawWire,
        Action::Rotate,
        Action::Delete,
    ],
    &[Action::AddPage],
    &[Action::ZoomIn, Action::ZoomOut, Action::ZoomFit, Action::ToggleGrid],
    &[Action::LibraryStatus, Action::LibraryUpdate],
    &[Action::Commit, Action::NewBranch, Action::History],
    &[Action::ExportPdf],
];

/// A named group of ribbon buttons.
pub type RibbonGroup = (&'static str, &'static [Action]);

/// Ribbon tabs, each with named groups.
pub const RIBBON: &[(&str, &[RibbonGroup])] = &[
    (
        "Start",
        &[
            ("Project", &[Action::Open, Action::Save, Action::ExportPdf]),
            ("Edit", &[Action::Undo, Action::Redo, Action::Delete]),
            (
                "Draw",
                &[Action::Select, Action::PlaceSymbol, Action::DrawWire, Action::Rotate],
            ),
        ],
    ),
    (
        "Pages",
        &[
            ("Pages", &[Action::AddPage]),
            (
                "View",
                &[Action::ZoomIn, Action::ZoomOut, Action::ZoomFit, Action::ToggleGrid],
            ),
        ],
    ),
    (
        "Library",
        &[("Library", &[Action::LibraryStatus, Action::LibraryUpdate])],
    ),
    (
        "Versions",
        &[("Versions", &[Action::Commit, Action::NewBranch, Action::History])],
    ),
    ("View", &[("Layout", &[Action::ToggleChrome])]),
];
