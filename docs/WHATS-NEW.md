# What's new

## Gozler 0.5.8

### Improved usability and rendering, plus in-app updates

#### Progress for several jobs at once

Export, Delete, updates and service repair each show their own progress in the status bar, alongside transfers and indexing. Each job has its own Cancel button when it can be stopped.

#### Selection masks and column fitting

- **Choose whether selection masks include folders.** Num + and Num − include matching folders by default. To select files only, change **Selection** under **Settings > General**. Canceling a mask keeps your previous selection.
- **Fit columns from the header.** Double-click a column divider to fit that column, or right-click a header to fit one or all columns.
- **See panel totals in the main status bar.** It shows the active panel's entry count, the size of selected items and the drive's free space.

#### Update Gozler from the app

When an update is available, choose **Help > Update** in Search or Commander, or click the update in the status bar.

#### New About dialog

The About dialog has a new layout with the app and service versions, plus links to Synapse, the source code and project credits.

#### Fixed

- Repeated PgUp/PgDn scrolling could mix or corrupt row text.
- Returning to a parent folder or receiving folder updates could leave the cursor out of view. The folder you left is now focused when you return, and a visible cursor stays in view as rows change.
- Folders sorted by Size could appear in one order and then jump when their sizes arrived. They now appear in size order from the start.
- A plain click in Commander selected a row. It now moves only the cursor. Ctrl+click adds or removes a row from the selection, and Shift+click selects a range. Folder updates preserve the range's starting point.
- Size and extension values could be cut off after navigation or scrolling. Columns now fit the visible values after the rows update.
- Filename wildcards such as `gozler*.exe` could fail with Match path enabled.

The installer upgrades Gozler 0.5.7 in place. Keep `gozler.exe`, `gozlerd.exe` and `gozler-ui.exe` from the same release.

[Full changelog](https://github.com/SynoSoftware/gozler/compare/v0.5.7...v0.5.8)
