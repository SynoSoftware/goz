//! The icons the window uses, from Windows' own icon font (DESIGN.md: Look):
//! Segoe Fluent Icons on Windows 11, or Segoe MDL2 Assets on Windows 10,
//! which draws the same code points. `win::gdi` draws them.
//!
//! Each variant is named exactly as Microsoft names its code point in
//! `docs/segoe-icons.tsv`, so the table finds it by name. Only the icons the
//! window uses are declared: a new one is one line here, picked by name from
//! that table among the rows whose Windows 10 column says "yes".

/// One icon of the Windows icon fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Glyph {
    /// A check mark: OK.
    Accept,
    /// A plus: Add….
    Add,
    /// A window coming back to the front: Show window, and the Keyboard
    /// page's group of the shortcut that shows Gozler.
    BackToWindow,
    /// A bulleted list: the Context menu page and its item group.
    BulletedList,
    /// A thin cross: Cancel, Close, Exit, and cancels a file operation.
    Cancel,
    /// A list with check marks: the Include page.
    CheckList,
    ChromeMaximize,
    ChromeRestore,
    Switch,
    /// An arrow head pointing down: hides the inspector, which sits at the
    /// bottom.
    ChevronDown,
    /// An arrow head pointing up: shows the inspector from the bottom.
    ChevronUp,
    /// An arrow pointing right, out of a bar at the left: shows the filter
    /// pane, which opens from the left.
    ClosePane,
    /// A console window: the Executables kind.
    CommandPrompt,
    /// A processor chip: the service's memory in the status bar.
    Component,
    /// Two pages: Copy, a copy in the File operations window, the File
    /// operations page's copying group, and the inspector's copy icons.
    Copy,
    /// A page with an arrow: Copy to folder….
    CopyTo,
    /// Scissors: Cut.
    Cut,
    /// A waste basket: Delete, and a delete in the File operations window.
    Delete,
    /// A page with lines of text: the Documents kind.
    Document,
    /// An arrow down onto a line: Update, Downgrade and Download.
    Download,
    /// A pencil: Edit… and Modify….
    Edit,
    /// An eraser: every Clear, which empties its box, and Clear filters.
    EraseTool,
    /// An arrow leaving a box: Export results.
    Export,
    /// A window with a title bar: the app's memory in the status bar.
    Favicon,
    /// A folder in a window: Explore.
    FileExplorer,
    /// A closed folder: Copy folder, the Folders kind, the folders of the
    /// status bar's totals and the Include page's folder group.
    Folder,
    /// An open folder: Open file location.
    FolderOpen,
    /// A globe: the General page's Language and display group.
    Globe,
    /// A hard drive: the Drives and service page and its drive group.
    HardDrive,
    /// A question mark: Search syntax.
    Help,
    /// An "i" in a circle: Properties and About.
    Info,
    /// A keyboard: the Keyboard page.
    KeyboardClassic,
    /// A keyboard key: the Keyboard page's group of command shortcuts.
    KeyboardShortcut,
    /// Books on a shelf: the group of what the index holds.
    Library,
    /// Two chain links: Copy path.
    Link,
    /// A page entering a folder: Move to folder…, and a move in the File
    /// operations window.
    MoveToFolder,
    /// Music notes: the Audio kind.
    MusicNote,
    /// A folder with a star: New folder….
    NewFolder,
    /// A page with an arrow: Open, the Context menu page's Open with group
    /// and the Search page's results group.
    OpenFile,
    /// An arrow pointing left, into a bar at the left: hides the filter
    /// pane, which closes to the left.
    OpenPane,
    /// A box: the Archives kind.
    Package,
    /// A blank page: the Files kind and the files of the status bar's
    /// totals.
    Page,
    /// A clipboard: Paste.
    Paste,
    /// Two bars: pauses a file operation.
    Pause,
    /// Two people: the Visibility page and its list.
    People,
    /// A framed picture: the Pictures kind.
    Picture,
    /// A triangle pointing right: resumes a paused file operation, and
    /// continues an interrupted one.
    Play,
    /// A power symbol: the General page's Starting group.
    PowerButton,
    /// Gears: the group of the service.
    Processing,
    /// A circular arrow: Refresh.
    Refresh,
    /// A minus: Remove, and Hide in the File operations window.
    Remove,
    /// A text cursor in a box: Copy name, and the Include page's name
    /// group.
    Rename,
    /// A diskette: Apply.
    Save,
    /// A magnifying glass: Find, Search in this folder, and the Search page
    /// and its search box group.
    Search,
    /// A square of dashed corners: Select all.
    SelectAll,
    /// A gear: Settings, and the General page.
    Settings,
    /// A speedometer: the File operations page.
    SpeedHigh,
    /// Two arrows in a circle: the File operations page's ring copy group.
    Sync,
    /// An arrow turning back: Reset and Reset all.
    Undo,
    /// A film camera: the Videos kind.
    Video,
    /// An eye: View.
    View,
    /// Four squares: the All kind.
    ViewAll,
}

impl Glyph {
    pub(crate) const ALL: [Glyph; 62] = [
        Glyph::Accept,
        Glyph::Add,
        Glyph::BackToWindow,
        Glyph::BulletedList,
        Glyph::Cancel,
        Glyph::CheckList,
        Glyph::ChromeMaximize,
        Glyph::ChromeRestore,
        Glyph::Switch,
        Glyph::ChevronDown,
        Glyph::ChevronUp,
        Glyph::ClosePane,
        Glyph::CommandPrompt,
        Glyph::Component,
        Glyph::Copy,
        Glyph::CopyTo,
        Glyph::Cut,
        Glyph::Delete,
        Glyph::Document,
        Glyph::Download,
        Glyph::Edit,
        Glyph::EraseTool,
        Glyph::Export,
        Glyph::Favicon,
        Glyph::FileExplorer,
        Glyph::Folder,
        Glyph::FolderOpen,
        Glyph::Globe,
        Glyph::HardDrive,
        Glyph::Help,
        Glyph::Info,
        Glyph::KeyboardClassic,
        Glyph::KeyboardShortcut,
        Glyph::Library,
        Glyph::Link,
        Glyph::MoveToFolder,
        Glyph::MusicNote,
        Glyph::NewFolder,
        Glyph::OpenFile,
        Glyph::OpenPane,
        Glyph::Package,
        Glyph::Page,
        Glyph::Paste,
        Glyph::Pause,
        Glyph::People,
        Glyph::Picture,
        Glyph::Play,
        Glyph::PowerButton,
        Glyph::Processing,
        Glyph::Refresh,
        Glyph::Remove,
        Glyph::Rename,
        Glyph::Save,
        Glyph::Search,
        Glyph::SelectAll,
        Glyph::Settings,
        Glyph::SpeedHigh,
        Glyph::Sync,
        Glyph::Undo,
        Glyph::Video,
        Glyph::View,
        Glyph::ViewAll,
    ];

    /// Its character in the icon fonts.
    pub(crate) fn code_point(self) -> char {
        match self {
            Glyph::Accept => '\u{E8FB}',
            Glyph::Add => '\u{E710}',
            Glyph::BackToWindow => '\u{E73F}',
            Glyph::BulletedList => '\u{E8FD}',
            Glyph::Cancel => '\u{E711}',
            Glyph::ChromeMaximize => '\u{E922}',
            Glyph::ChromeRestore => '\u{E923}',
            Glyph::Switch => '\u{E8AB}',
            Glyph::CheckList => '\u{E9D5}',
            Glyph::ChevronDown => '\u{E70D}',
            Glyph::ChevronUp => '\u{E70E}',
            Glyph::ClosePane => '\u{E89F}',
            Glyph::CommandPrompt => '\u{E756}',
            Glyph::Component => '\u{E950}',
            Glyph::Copy => '\u{E8C8}',
            Glyph::CopyTo => '\u{F413}',
            Glyph::Cut => '\u{E8C6}',
            Glyph::Delete => '\u{E74D}',
            Glyph::Document => '\u{E8A5}',
            Glyph::Download => '\u{E896}',
            Glyph::Edit => '\u{E70F}',
            Glyph::EraseTool => '\u{E75C}',
            Glyph::Export => '\u{EDE1}',
            Glyph::Favicon => '\u{E737}',
            Glyph::FileExplorer => '\u{EC50}',
            Glyph::Folder => '\u{E8B7}',
            Glyph::FolderOpen => '\u{E838}',
            Glyph::Globe => '\u{E774}',
            Glyph::HardDrive => '\u{EDA2}',
            Glyph::Help => '\u{E897}',
            Glyph::Info => '\u{E946}',
            Glyph::KeyboardClassic => '\u{E765}',
            Glyph::KeyboardShortcut => '\u{EDA7}',
            Glyph::Library => '\u{E8F1}',
            Glyph::Link => '\u{E71B}',
            Glyph::MoveToFolder => '\u{E8DE}',
            Glyph::MusicNote => '\u{EC4F}',
            Glyph::NewFolder => '\u{E8F4}',
            Glyph::OpenFile => '\u{E8E5}',
            Glyph::OpenPane => '\u{E8A0}',
            Glyph::Package => '\u{E7B8}',
            Glyph::Page => '\u{E7C3}',
            Glyph::Paste => '\u{E77F}',
            Glyph::Pause => '\u{E769}',
            Glyph::People => '\u{E716}',
            Glyph::Picture => '\u{E8B9}',
            Glyph::Play => '\u{E768}',
            Glyph::PowerButton => '\u{E7E8}',
            Glyph::Processing => '\u{E9F5}',
            Glyph::Refresh => '\u{E72C}',
            Glyph::Remove => '\u{E738}',
            Glyph::Rename => '\u{E8AC}',
            Glyph::Save => '\u{E74E}',
            Glyph::Search => '\u{E721}',
            Glyph::SelectAll => '\u{E8B3}',
            Glyph::Settings => '\u{E713}',
            Glyph::SpeedHigh => '\u{EC4A}',
            Glyph::Sync => '\u{E895}',
            Glyph::Undo => '\u{E7A7}',
            Glyph::Video => '\u{E714}',
            Glyph::View => '\u{E890}',
            Glyph::ViewAll => '\u{E8A9}',
        }
    }

    /// The number a Windows control keeps for it as an item's data, which
    /// is never 0, so 0 means no icon.
    pub(crate) fn data(self) -> usize {
        Glyph::ALL
            .iter()
            .position(|glyph| *glyph == self)
            .expect("every glyph is in ALL")
            + 1
    }

    /// The glyph whose [`Self::data`] is `data`, if any.
    pub(crate) fn from_data(data: usize) -> Option<Glyph> {
        data.checked_sub(1).and_then(|i| Glyph::ALL.get(i)).copied()
    }
}
