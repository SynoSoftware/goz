# Stage 6: wiring the Viewer

The Viewer is written in four new files. This list connects them to the
existing code. Apply every item. Each item gives the file, where the change
goes, and the exact text.

New files, not to be edited for the wiring:

- `crates/gozler-winfs/src/view.rs`: `open_to_view`, and `Entry::handle_value`.
- `crates/gozler-ui/src/core/viewer_window.rs`: what a Viewer shows, its keys
  and menus. Its tests drive it with byte slices.
- `crates/gozler-ui/src/win/viewer.rs`: the area control, its painting, and
  the reading thread (`FileReader`).
- `crates/gozler-ui/src/app/viewer.rs`: `Viewers`, which the main window
  holds, and each Viewer window.

`core/viewer.rs` needs no change.

## 1. `mod` lines

`crates/gozler-winfs/src/lib.rs`: after `mod storage;` (with its
`#[cfg(windows)]`):

```rust
#[cfg(windows)]
mod view;
```

and after `pub use entry::{...};`:

```rust
#[cfg(windows)]
pub use view::open_to_view;
```

`crates/gozler-ui/src/core/mod.rs`: after `pub(crate) mod update;`:

```rust
pub(crate) mod viewer;
pub(crate) mod viewer_window;
```

`crates/gozler-ui/src/win/mod.rs`: after `pub(crate) mod update;`:

```rust
pub(crate) mod viewer;
```

`crates/gozler-ui/src/app/mod.rs`: after `mod status_bar;`:

```rust
mod viewer;
```

## 2. Items that become visible to `win::viewer`

`crates/gozler-ui/src/win/folder.rs`: make these `pub(super)`. Change nothing
else in them.

- `struct Stop` becomes `pub(super) struct Stop`, and its methods `new`,
  `enter`, `stopped` and `stop` become `pub(super) fn`.
- `fn verbatim`, `fn reason`, `fn register` and `fn unregister` become
  `pub(super) fn`.

`crates/gozler-ui/src/win/gdi.rs`: `fn muted_text() -> SYS_COLOR_INDEX`
becomes `pub(super) fn muted_text() -> SYS_COLOR_INDEX`. The Viewer draws a
hex row's offset in it.

## 3. `win/locale.rs`: the ANSI code page

The Viewer decodes a text file without a byte order mark that is not UTF-8
in Windows' ANSI code page. `console_text` already holds the conversion, so
it becomes one function for both code pages. Replace `console_text` with:

```rust
/// `bytes` in the console's code page, in which a program such as `net`
/// writes to a file, as text.
pub(crate) fn console_text(bytes: &[u8]) -> String {
    code_page_text(CP_OEMCP, bytes)
}

/// `bytes` in Windows' ANSI code page, as the Viewer shows a text file that
/// has no byte order mark and is not UTF-8.
pub(crate) fn ansi_text(bytes: &[u8]) -> String {
    code_page_text(CP_ACP, bytes)
}

fn code_page_text(code_page: u32, bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    // SAFETY: the first call measures; the second writes at most the
    // buffer's length.
    unsafe {
        let length = MultiByteToWideChar(code_page, flags, bytes, None);
        let mut text = vec![0u16; usize::try_from(length).unwrap_or(0)];
        let written = MultiByteToWideChar(code_page, flags, bytes, Some(&mut text));
        String::from_utf16_lossy(&text[..usize::try_from(written).unwrap_or(0)])
    }
}
```

Add `CP_ACP` to the `windows::Win32::Globalization` import.

## 4. `win/window.rs`: the Viewer's keys

A Viewer takes its own keys. A key pressed in it goes first to the Viewer as
`Message::KeyDown`, then to `IsDialogMessageW` of the Viewer, which gives
Retry its access key and `Enter`, and `Tab` its order. No key of a Viewer
reaches the main window's accelerators.

1. Add `RefCell` to `use std::cell::Cell;`, so it reads
   `use std::cell::{Cell, RefCell};`.
2. In the `thread_local!` block, after `MODELESS`, add:

   ```rust
       /// The windows that [`own_keys`] names.
       static OWN_KEYS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
   ```

3. After `fn set_modeless`, add:

   ```rust
   /// Names `window`, a top-level window besides the main one, as one that
   /// takes its own keys, or, with `own` false, no longer, as a Viewer does
   /// while it is open (file-manager-spec.md: The Viewer). The message loop
   /// offers each key pressed in it to the window first, as
   /// [`Message::KeyDown`], then to its `IsDialogMessageW` for `Tab` and the
   /// access keys among its controls.
   pub(crate) fn own_keys(window: Hwnd, own: bool) {
       OWN_KEYS.with_borrow_mut(|windows| {
           windows.retain(|named| *named != window.0);
           if own {
               windows.push(window.0);
           }
       });
   }
   ```

4. In `run_message_loop`, the branch `if root != main.0 {` starts with this
   block, before `let modeless = MODELESS.get();`:

   ```rust
                   if OWN_KEYS.with_borrow(|windows| windows.contains(&root)) {
                       if msg.message == WM_KEYDOWN
                           && SendMessageW(
                               root,
                               KEY_DOWN,
                               Some(msg.wParam),
                               Some(LPARAM(msg.hwnd.0 as isize)),
                           )
                           .0 != 0
                       {
                           continue;
                       }
                       if !IsDialogMessageW(root, &msg).as_bool() {
                           let _ = TranslateMessage(&msg);
                           DispatchMessageW(&msg);
                       }
                       continue;
                   }
   ```

5. In the doc comment of `run_message_loop`, replace "and one for any other
   window, such as a tooltip, is only dispatched." with "A message for a
   window that [`own_keys`] names, or for one of its controls, goes to that
   window's keys first and then its `IsDialogMessageW`, and one for any
   other window, such as a tooltip, is only dispatched."
6. The doc comment of `const KEY_DOWN` becomes: "The private message that
   offers a key to the main window, or to a window that [`own_keys`] names
   ([`Message::KeyDown`])."

`WAKE + 9` is the Viewer's `AREA_PAINT` (`win/viewer.rs`). Before you apply
this list, check that no other code took `WAKE + 9` meanwhile.

## 5. `core/layout.rs`: two widths

The Viewer's Retry and its status bar's right part need two widths whose
constants are private to `layout.rs`. After `pub(crate) fn button_room`, add:

```rust
/// The width of a push button whose caption is `caption` wide, with an icon
/// before it, as Retry under a message has.
pub(crate) fn image_button_width(caption: Px, dpi: Dpi) -> Px {
    button_width(button_content(caption, true, dpi), dpi)
}

/// The width of a status bar's last part, for a text `text` wide: the
/// part's borders and inset, and the size grip.
pub(crate) fn status_last_part(text: Px, dpi: Dpi) -> Px {
    text + STATUS_PART_PADDING.to_px(dpi) + SCROLL_BAR.to_px(dpi)
}
```

## 6. `resources/ids.h`

After the File operations window's block (`IDC_FILE_OPERATIONS_HIDE 3025`),
add:

```c
// The Viewer, a window of its own for each file it shows: the message and
// Retry, which lie above the area that shows the file, then the status bar.
#define IDC_VIEWER_MESSAGE 3501
#define IDC_VIEWER_RETRY 3502
#define IDC_VIEWER_AREA 3503
#define IDC_VIEWER_STATUS 3504
```

Check that 3501 to 3504 are still free. `build.rs` makes them
`ControlId::ViewerMessage`, `ViewerRetry`, `ViewerArea` and `ViewerStatus`.
No dialog resource is needed.

## 7. Catalogues

Add a `"viewer"` section after `"key_bar"` in each catalogue. The Viewer also
uses these existing keys: `panel.reading`, `panel.retry`,
`panel.cannot_open`, `menu.file` and `menu.view`.

`en.json`:

```json
  "viewer": {
    "title": "{0} – View",
    "close": "&Close",
    "text": "&Text",
    "hex": "&Hex",
    "wrap": "&Wrap long lines",
    "removed": "The drive was removed.",
    "cannot_read": "Windows could not read {0}: {1}",
    "status_text": "{0} · {1} · {2}",
    "status_hex": "{0} · {1}",
    "top": "Top",
    "end": "End",
    "percent": "{0} %"
  },
```

`es.json`:

```json
  "viewer": {
    "title": "{0} – Ver",
    "close": "&Cerrar",
    "text": "&Texto",
    "hex": "&Hexadecimal",
    "wrap": "&Ajustar líneas largas",
    "removed": "Se quitó la unidad.",
    "cannot_read": "Windows no pudo leer {0}: {1}",
    "status_text": "{0} · {1} · {2}",
    "status_hex": "{0} · {1}",
    "top": "Inicio",
    "end": "Final",
    "percent": "{0} %"
  },
```

`nl.json`:

```json
  "viewer": {
    "title": "{0} – Weergeven",
    "close": "&Sluiten",
    "text": "&Tekst",
    "hex": "&Hex",
    "wrap": "&Lange regels afbreken",
    "removed": "Het station is verwijderd.",
    "cannot_read": "Windows kan {0} niet lezen: {1}",
    "status_text": "{0} · {1} · {2}",
    "status_hex": "{0} · {1}",
    "top": "Begin",
    "end": "Einde",
    "percent": "{0}%"
  },
```

`tr.json`:

```json
  "viewer": {
    "title": "{0} – Görüntüle",
    "close": "&Kapat",
    "text": "&Metin",
    "hex": "&Onaltılık",
    "wrap": "&Uzun satırları kaydır",
    "removed": "Sürücü çıkarıldı.",
    "cannot_read": "Windows {0} öğesini okuyamadı: {1}",
    "status_text": "{0} · {1} · {2}",
    "status_hex": "{0} · {1}",
    "top": "Baş",
    "end": "Son",
    "percent": "%{0}"
  },
```

In `viewer.title`, `{0}` is the file's name. In `cannot_read`, as in
`panel.cannot_open`, `{0}` is the file's name and `{1}` is Windows' reason.
In `status_text`, `{0}` is the name, `{1}` the encoding and `{2}` the size.

## 8. Settings: `core/settings.rs`

After `pub(crate) struct Placement { ... }`, add:

```rust
/// What the Viewer remembers (file-manager-spec.md: The Viewer): the size
/// and place of the Viewer that was moved or closed last, and Wrap long
/// lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ViewerState {
    #[serde(
        default,
        deserialize_with = "placement",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) placement: Option<Placement>,
    #[serde(default = "on", deserialize_with = "on_or_default")]
    pub(crate) wrap: bool,
}

impl Default for ViewerState {
    fn default() -> ViewerState {
        ViewerState {
            placement: None,
            wrap: on(),
        }
    }
}
```

In `pub(crate) struct WindowState`, after `panels`, add:

```rust
    /// The Viewer's size and place, and Wrap long lines.
    #[serde(default, deserialize_with = "or_default")]
    pub(crate) viewer: ViewerState,
```

In `impl Default for WindowState`, after `panels: SavedPanels::default(),`,
add `viewer: ViewerState::default(),`.

In the settings round-trip test (the `WindowState { ... }` literal that ends
with `panels: SavedPanels { ... },`), add a value that differs from the
default, so the test watches that the Viewer's state is saved and read back:

```rust
                viewer: ViewerState {
                    placement: Some(Placement {
                        left: 40,
                        top: 30,
                        right: 940,
                        bottom: 730,
                        maximized: true,
                    }),
                    wrap: false,
                },
```

## 9. Model: `core/model.rs`

Add `ViewerState` to `use super::settings::{...}`. After
`pub(crate) fn placement(&self)`, add:

```rust
    /// What the Viewer remembers, which the next Viewer opens with.
    pub(crate) fn viewer(&self) -> ViewerState {
        self.settings.window.viewer
    }

    pub(crate) fn set_viewer(&mut self, state: ViewerState) {
        self.settings.window.viewer = state;
    }
```

## 10. The panel's focused entry: `core/panel.rs`

In `impl Panel`, after `pub(crate) fn entry`, add:

```rust
    /// The focused entry, which View opens; never `..`.
    pub(crate) fn focused_entry(&self) -> Option<&Entry> {
        self.focused.and_then(|row| self.entry(row))
    }
```

`chosen` and `chosen_count` each repeat `self.focused.and_then(|row|
self.entry(row))`; make them call `focused_entry()`.

## 11. The View command: `core/commands.rs`

Replace

```rust
                // The Viewer comes in stage 6 of file-manager-spec.md.
                C::View => ("command.view", &[], never, None),
```

with

```rust
                C::View => (
                    "command.view",
                    &[],
                    |s| {
                        let panels = s.model.panels();
                        panels
                            .panel(panels.active())
                            .focused_entry()
                            .is_some_and(|entry| !entry.is_folder())
                    },
                    None,
                ),
```

`F3` already runs `CommandId::View` in Two panels, and the key bar's
`F3 View` follows the row's state.

## 12. The main window: `app/main_window.rs`

1. Add `use super::viewer::Viewers;` beside `use super::file_operations::FileOperations;`.
2. In `struct MainWindow`, after `file_operations`, add:

   ```rust
       /// The open Viewers (file-manager-spec.md: The Viewer).
       viewers: Viewers,
   ```

3. In `MainWindow::open`, before `let window = Rc::new(MainWindow {`, add
   `let viewers = Viewers::new(model.viewer());`, and in the struct literal,
   after `file_operations: RefCell::new(None),`, add `viewers,`.
4. In `run`, replace

   ```rust
                   // Not available in this version (file-manager-spec.md: The
                   // Viewer).
                   CommandId::View | CommandId::Edit => return,
   ```

   with

   ```rust
                   CommandId::View => return self.view_focused(hwnd),
                   // Not available in this version (file-manager-spec.md:
                   // Commands in Two panels).
                   CommandId::Edit => return,
   ```

5. After `fn open_file_operations`, add:

   ```rust
       /// `F3`: opens a Viewer of the active panel's focused file
       /// (file-manager-spec.md: The Viewer).
       fn view_focused(&self, hwnd: Hwnd) {
           let (path, strings) = {
               let model = self.model.borrow();
               let panels = model.panels();
               let panel = panels.panel(panels.active());
               let Some(entry) = panel.focused_entry().filter(|entry| !entry.is_folder()) else {
                   return;
               };
               (panel.path_of(&entry.name), model.strings().clone())
           };
           if let Err(error) = self.viewers.open(path, strings, hwnd) {
               super::write_log(
                   &self.settings_file.with_file_name("ui-error.log"),
                   &format!("The Viewer did not open: {error}"),
               );
           }
       }
   ```

6. Exit closes every Viewer. In `fn exit`, before
   `if self.instance == Instance::First { self.save_settings(hwnd, true); }`,
   add `self.viewers.close_all();`. Each Viewer leaves its size, place and
   Wrap long lines in `Viewers` as it closes, so the save that follows
   holds them. Closing the main window to the notification area does not
   call `exit`, so the Viewers stay open.
7. In `Message::Destroy`, before `win::window::quit();`, add
   `self.viewers.close_all();`, so a main window destroyed by another path
   leaves no Viewer behind a message loop that has ended.
8. In `fn save_settings`, before the `self.model.borrow_mut().save_settings(...)`
   call, add:

   ```rust
           self.model.borrow_mut().set_viewer(self.viewers.state());
   ```

9. In `fn set_language`, after the `file_operations` block, add:

   ```rust
           self.viewers.set_strings(&Strings::load(language));
   ```

10. Where Settings applies `show_icons` (after `operations.apply_glyphs();`
    inside `if edited.choices.show_icons != last.choices.show_icons`), add
    `self.viewers.apply_glyphs();` after the `file_operations` block.

Each Viewer handles `WM_SYSCOLORCHANGE`, its DPI and the regional format
itself, because Windows sends them to every top-level window.

## Checks after applying

- `check.ps1` runs `viewer_window`'s four tests and `view.rs`'s test.
- In the window: `F3` on a file in a panel opens a Viewer; `Esc` or `F3`
  closes it and the main window shows the same focused row; `F3` on a folder
  does nothing and `F3 View` is disabled there; File > Exit closes every
  open Viewer.
