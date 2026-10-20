# Service settings

This plan adds the two service settings that the window does not have yet,
because each needs a service request that does not exist yet:

- Dates created, a check box on the Drives and service page, which adds the
  Date created column and the `dc:` search function;
- Rebuild index, a button on the Drives and service page.

It is scheduled after the Include page, which `docs/DESIGN.md` now describes
("Include settings" and "The Include page"). These settings follow the same
rules: they are saved for the whole machine in `HKLM\SOFTWARE\Gozler`, only an
administrator changes them, OK and Apply show the administrator shield, and
the service applies a change without a restart. The plan owns how a request
that acts at once, such as Rebuild index, proves that an administrator sent
it.

The approved prototype of these screens, with their English words, is in git
history: `git show b43f935:docs/plans/ui-prototype.html`, a page that opens in
a browser. Its Settings dialog shows the Index page, now Drives and service,
with both controls.

## The Drives and service page

- **Dates created** is off by default, because it adds memory per entry to the
  service. The page shows the service's memory, so the person sees the cost
  after they turn it on. It is a search feature, so `gozler` gets `dc:` in the
  same change, because the window adds no search feature that `gozler` lacks.
- **Rebuild index** makes the service read every included drive again, as
  after a restart. It acts at once, so the button itself has the
  administrator shield and asks Windows for permission. It is for an index
  the person no longer trusts; the service keeps itself current without it.
  While it runs, the drives show "indexing" and searches give incomplete
  results, as the status bar's service group says.
- **Users and groups who see every file** is a list with Add and Remove and
  an "Every user sees every file" check box, as the
  [visibility spec](visibility-spec.md) `V14` describes. It needs the
  visibility plan's stored list, so it comes after that plan.

They take over two of Everything's options, on its Indexes page: Force
Rebuild becomes Rebuild index, and Index date created becomes Dates created.

## The Date created column

The change that adds the Date created column also gives each column one row,
as `CommandId::row` does for a command. Three of `Column`'s facts in
`core/model.rs` (`shown_by_default`, `can_hide`, `right_aligned`) are
`matches!` or `==`, so a new column silently gets "shown by default, can be
hidden, left-aligned". Date created must choose its default on purpose,
because it costs service memory and its index setting is off by default.
`Column::row(self) -> ColumnRow` is one exhaustive match, and
`ColumnRow { header, menu_label, width: Dip, shown_by_default: bool, can_hide:
bool, right_aligned: bool, sort_key: Option<SortKey>, export_column:
Option<EsColumn> }` replaces the per-column functions. `text::cell` stays
apart, because it formats data. The names that `ui.json` saves do not change,
so saved settings still load. No new test: the compiler forces each row.

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
