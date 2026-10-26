# Service settings

This plan adds the service setting that the window does not have yet:
Users and groups who see every file, a list, which the
[visibility plan](visibility-plan.md) stores. It gets a Settings page of its
own, Visibility, right after Include, because the Drives and service page
has no room: the list would make the Settings dialog about 35 DLU taller,
and its bottom edge would then go under the taskbar on a 1080p screen at
125 % scaling.

Rebuild index and Dates created, the Drives and service page's settings,
are built: `docs/DESIGN.md` describes them in "Include settings" and "The Drives and
service page".

**Stages.** The list is its own stage, the visibility plan's stage 3. It
starts at once and lands right after that plan's stage 2, because the public
release that turns the visibility filter on for the other users of a shared
PC also carries the page where an administrator changes who sees every file
(visibility plan, "Schedule").

The list follows the Include page's rules: it is saved for the whole machine
in `HKLM\SOFTWARE\Gozler`, only an administrator changes it, OK and Apply
show the administrator shield, and the service applies a change without a
restart. OK and Apply save every changed service setting through one
elevated copy of the window, as `gozler-ui --save-include` saves Include and
Dates created today, which then writes each changed value; so the list adds
a value, never a save path of its own.

The approved prototype of these screens, with their English words, is in git
history: `git show b43f935:docs/plans/ui-prototype.html`, a page that opens in
a browser. Its Settings dialog shows the Index page, now Drives and service.

## The Visibility page

- **Users and groups who see every file** is a list with Add… and Remove and
  an "Every user sees every file" check box above it, as the
  [visibility spec](visibility-spec.md) `V14` describes. Add… opens Windows'
  "Select Users or Groups" dialog, the one Windows' own permission editors
  use. The check box is checked when the list holds Everyone, and checking
  or clearing it adds or removes Everyone; it stores nothing of its own.
  Anyone may read the list. It is the `REG_MULTI_SZ` value `SeeEveryFile`
  under `HKLM\SOFTWARE\Gozler`, beside `Include`, one SID per line, the same
  value that `gozlerd see-every-file` and the setup write. The service reads
  it for each search, so the page sends no request after saving it. The
  stage's After install list holds the screenshots of the list, empty and
  with entries, and of the Select Users or Groups dialog it opens.

## After install: the Include page

The Include page shipped with protocol 8, and the installed service speaks an
older protocol, so the owner checks these after installing, with Windows'
permission prompt:

- the Include page and Indexed folders look as the prototype shows them, and
  the tree's partial check boxes show where folders below differ;
- unchecking a folder removes its entries, and `gozler` finds none of them;
- checking it again shows the drive indexing, then finds them again;
- a name pattern and Exclude hidden take effect at once;
- unchecking a drive shows it as "Left out" and removes it from Location;
- saying no to the permission prompt keeps the dialog open with the edits.
