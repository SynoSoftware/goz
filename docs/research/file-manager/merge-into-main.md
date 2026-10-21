# Merging `file-manager` into main

The owner merges. This note lists what the merge needs. Main was at
`7b94f06` when this was written, with uncommitted work on top.

## What `file-manager` adds since main's `7b94f06`

| Commit | Change |
| --- | --- |
| `6bb243a` | Windows draws the panel rows; a search panel reads its selection once per change |
| `3a63ad7` | merge of `7b94f06` into the branch |
| `a680253` | Windows draws the Settings section headings, dividers, Location list, Failed list path column and File operations card |
| `8b02d66` | Windows draws the status bar's right group (#109) |
| `7be252f` | fixes from the final review: focus outline, Viewer after a pulled drive, events in order, failed opens, a renamed shown folder, `..` icon, taken-entry filter |
| `f34fbc0` | stage 8: DESIGN.md chapters; every spec citation in code points to DESIGN.md |
| `dbc2f10` | the Two panels prototype, kept in history |
| `66ed9ef` | stage 8 finished: File operations page, Search's commands and menus, Cut and Paste, CONTEXT.md terms; the spec and the prototype deleted |
| `b277fa4` | fix round 2: taken names folded once (#112), the `..` arrow back, a panel's status line counts unknown sizes |

`docs/DESIGN.md` conflicts with main's uncommitted edit of the Search
status bar's totals: keep main's partial-answer wording ("Each size sums
the known sizes and says how many are unknown …"); the branch's panel
status line already uses the same form.

## Files both sides change

Main's side counts its commits after `7b94f06` and the uncommitted work
in the main checkout when this was written. Expect conflicts in:

- `crates/gozler-ui/src/app/file_operations.rs`
- `crates/gozler-ui/src/app/main_window.rs`
- `crates/gozler-ui/src/app/main_window/panels.rs`
- `crates/gozler-ui/src/core/captions.rs`
- `crates/gozler-ui/src/core/file_operations.rs`
- `crates/gozler-ui/src/core/model.rs`
- `crates/gozler-ui/src/core/panel.rs`
- `crates/gozler-ui/src/win/file_operations.rs`
- `crates/gozler-ui/src/win/window.rs`
- `docs/DESIGN.md`

Things to keep from `file-manager` when resolving:

- `layout::DIVIDERS` is 4: Two panels has a line between the panels, one
  above the key bar and one above each panel's status line.
- `Message::EraseBackground` and `Message::Paint` are gone; those
  messages reach `DefWindowProc` through `Message::Other`.
- `register_class` takes a `Background`; the Viewer uses
  `Background::Window`.
- Every comment that cited `docs/plans/file-manager-spec.md` now cites a
  DESIGN.md section. Main's new comments must not cite the spec, which
  this branch deletes.

## Open findings

- #110: window waits found by reading (operations.json saved on the
  window thread, failure dialog rows, per-folder passes).
- #111: eleven small defects from the final review.
- A panel notices a rename of its shown folder by another program within
  2 seconds; when `FileIdInfo` fails at listing time (not checked on FAT
  or exFAT), it does not notice.

## After install

What only the owner can check, in the installed build:

1. Start Gozler with the mouse, open Two panels and press only arrow
   keys: the focused row shows its outline.
2. The panel rows, the `..` row's icon (a folder with an up arrow), and
   a taken entry struck through in grey in a search panel.
3. Settings: each section is a heading with an icon, the controls are
   indented under it, and an etched line separates sections.
4. The status bar's right group: the index state, the files total and
   the folders total each in its own part with its icon; narrow the
   window and a total hides whole, never cut.
5. Open a file in the Viewer from a USB drive and pull the drive out:
   the Viewer says the drive was removed and Retry reads it again after
   the drive is back.
6. Rename the folder a panel shows from File Explorer: within 2 seconds
   the panel opens the nearest folder that exists.
7. A folder size that is unknown: the panel's status line says "size
   unknown".
