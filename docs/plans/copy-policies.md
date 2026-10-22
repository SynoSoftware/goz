# Copy policies

This plan changes what a copy or a move does with a file that already
exists at the destination. Today the window asks one question after prepare
and before anything is written. For a folder that merges, that question is
about files that prepare never read: "The destination already has 1 folder
with the same name. Your choice applies to the files inside it."

**Owner ruling: the work never stops for a question.** The person chooses a
policy before the copy starts, and the copy applies it. Where the policy is
Ask, a file that needs an answer is set aside and everything else goes on.
Total Commander asks one question after another during a copy, so a copy
left running for hours stops at the first one, and that is the failure this
plan removes.

**Schedule.** It starts after the disk-full fix that names the file and the
room it needs lands, because both change `gozler-fileops`. It is not tied to
a release.

## What the person sees

- **A folder with the same name merges** without a question, as File
  Explorer, Total Commander and Robocopy merge it.
- **Settings > File operations has "If a file exists"**, a drop-down list
  above the speed settings, saved in `ui.json`:

  | Choice | A file that already exists |
  | --- | --- |
  | Ask me (the default) | set aside; the window asks, and the rest of the copy goes on |
  | Replace if newer | the same size and last write time: left as it is, counted done. The source is newer: replaced. The destination's file is newer: kept, and listed as "The file in the destination is newer." |
  | Replace if different | the same size and last write time: left as it is, counted done. Any other difference: replaced, also when the destination's file is newer. This is Robocopy's default rule |
  | Replace | replaced |
  | Skip | kept, and listed as skipped |
  | Keep both | the new file gets the first free number: `report (2).pdf` |

  One sentence under the list says what the chosen policy does, as the
  speed presets have. "The same size and last write time" is the engine's
  existing rule for a file that is done (`conflicts::is_done`: 2 seconds,
  and one hour for a daylight saving change, because FAT keeps local times
  to 2 seconds). A damaged value in `ui.json` falls back to Ask me.
- **The Copy and Move dialog shows the policy** in one line under the edit
  box, such as "If a file exists: Ask me", with an Options button that
  opens Settings at the File operations page. A change there applies to
  this copy when the dialog's Copy or Move is chosen after it. Search's
  folder picker shows the same line; see "Load-bearing assumptions".
- **No question before the copy.** Prepare no longer leads to the conflict
  question, so Copy or Move in the dialog starts the work.
- **A set-aside file shows at once.** As soon as the run sets one aside,
  the operation's row in the File operations window says "1 file needs your
  answer", and the window shows the question in its top area under the
  running operation: the files, each with its size and date at both ends,
  and Replace, Skip, Keep both, Replace if newer and Replace if different,
  with a "Remember my answer" check box. The question never takes the
  keyboard focus and never opens a dialog of its own, and the status bar's
  middle group says "Copying Projects to D:\Backup · 1 file needs your
  answer". The window opens for it as it opens for a long operation (see
  DESIGN.md "File operations"), also after Hide.
- **The answer** applies to every file set aside so far and to every later
  conflict of this operation, which then runs as if it had started with
  that policy. With "Remember my answer", it also becomes the policy in
  Settings. The prototype decides whether the person can also answer file
  by file (stage 1).
- **An operation whose other work is done** stays in the queue, waiting for
  the answer, with its set-aside files. It holds its paths, so an operation
  that reads or writes them waits for it, as two operations on one path wait
  today (D14); every other operation runs.
- **Cancel** ends the operation; its set-aside files are listed as
  cancelled in the end's list. **Exit** with an operation waiting for an
  answer saves it, and Continue at the next start meets the same files again
  and sets them aside again.
- **In Two panels, a running copy docks in the key bar's place.** While an
  operation runs, the row of F3 View to F8 Delete buttons gives way to a
  docked bar: the operation's title and destination, such as "Copying
  Projects to D:\Backup", its bar and percent, and "1 file needs your
  answer" when one does. A click on it shows the File operations window, and
  that window's Hide collapses it back into the docked bar. The `F` keys
  keep working while the bar is docked; only their buttons give way. The key
  bar returns when the last operation ends. With several operations the bar
  shows the newest running one and how many others run, as the status bar's
  middle group does. Search has no key bar, so there the status bar's middle
  group stays the docked form.
- **Failures stay failures.** Access denied, a locked file or a full disk
  get no question: there is no useful answer while the copy runs. They are
  listed at the end as today. Paths longer than 260 characters work, because
  the engine opens every path with `\\?\`.

## What the engine does

- `Choice` gains the two new rules. The run's rule keeps
  `choice: Option<Choice>`, and `None` now means Ask: `decide` returns a new
  decision that sets the entry aside instead of skipping it, and it takes no
  name, so a later answer decides the name then.
- Two selected entries with one name follow the same rules as a file that
  exists: with Ask they are set aside, and Replace still never makes one
  replace the other; the second gets a number.
- `Queue` gains one call that gives an operation its answer. The run thread
  of an operation whose walk ended with files set aside waits on that
  answer, on its own thread, so nothing else waits; Cancel and Exit wake it.
  The set-aside files then go through `decide` again with the answer, under
  the folder's name lock, so a name taken meanwhile is still caught.
- `State` gains how many files wait for an answer, and the files
  themselves, for the window's list: the source, the destination, and the
  size and last write time at both ends.
- The saved operation records the operation's choice as today, now any of
  the five. A saved file from an earlier version loads as before, and Ask is
  saved as no choice, which it already is.
- What nothing reads any more is removed in the same change: the
  `Conflicts` counts that only fed the question, the question's catalogue
  keys and dialog code, and the rule that skips a conflict nobody was asked
  about.

## Load-bearing assumptions

Check each before the stage that depends on it, from the code or the
documentation, and record the answer in this plan.

1. **Search's folder picker can show the line.** Search asks with Windows'
   folder picker. `IFileDialogCustomize` adds a drop-down list or a static
   text to it. If it cannot, Search shows the policy only in Settings, and
   this plan says so.
2. **A run thread can wait without holding a lock or a worker.** The walk's
   workers and the ring must be finished or idle while the run thread waits,
   so a waiting operation costs no I/O and no CPU.
3. **The File operations window can show the question without a new painted
   surface.** It must be built from Win32 controls (a list view, buttons, a
   check box), because a new painted surface needs the owner's permission.
4. **The docked bar fits the key bar's row** with Win32 controls: a static
   text, a progress bar control and the controls the prototype settles on,
   at every DPI. A click on a static text or a progress bar needs
   `SS_NOTIFY` or a button that holds them; the stage records which.

## Stages

One implementer keeps its context through every stage and fixes the
review's findings; the reviewer reads and reports.

1. **Prototype.** Run `/prototype` before any production code changes.
   Start from the File operations window's approved prototype, not from a
   blank page: restore it with
   `git show a88e6a6:crates/gozler-ui/prototypes/file-operations-prototype.html`
   to its old path and extend it, keeping what it already settled. Use both
   of `/prototype`'s branches, as tabs of that one page:
   - **Logic.** One HTML page that drives the state model: an operation with
     sample files, each policy, files set aside during the walk, an answer
     given early, an answer given after the rest is done, Remember my
     answer, Cancel, and Exit followed by Continue. It shows the whole state
     after every step.
   - **UI.** Side-by-side variants of the four screens, in their English
     words: the Copy and Move dialog's policy line, Settings > File
     operations with "If a file exists", the File operations window with
     the question, with one file and with 200, and Two panels with the
     docked bar in the key bar's place, with one operation, with three, and
     with a file that needs an answer. Draw only Win32 controls, because the
     window is built from them.

   The owner reviews it, and the owner's verdict and what it settled go into this
   plan. Stage 2 does not wait for the review; stages 3 to 5 do, because
   they build the screens.
2. **Engine.** The policies, setting aside, the answer call, the new state
   and the saved choice. Proof: unit tests of `decide` with one case per
   rule and per boundary (the same file, a newer source, a newer
   destination, the same time with another size), a test that a saved file
   of an earlier version still loads, and a real-folder test that a run with
   Ask sets a file aside, finishes the rest, and applies a later answer.
3. **Window.** The question in the File operations window, the status bar's
   middle group, the removal of the conflict question, and Remember my
   answer. Proof: code review of the question's flow; screenshots of the
   question with one file and with 200, saved for the owner.
4. **Settings and the Copy dialog.** "If a file exists" on the File
   operations page, the policy line with Options in the Copy and Move
   dialog, and in Search's folder picker if assumption 1 holds. Proof: a
   test that a damaged value falls back to Ask me; screenshots of the page
   and both dialogs.
5. **The docked bar.** The bar in the key bar's place, its click, Hide
   collapsing the window into it, and the key bar's return. Proof: code
   review of when the bar and the key bar swap; screenshots with one
   operation, with three, and with a file that needs an answer.
6. **Documents.** DESIGN.md "File operations", "Copy and Move", "The
   File operations page" and the key bar, and the engine spec's
   "Conflicts", describe what this plan built. The plan and the prototype
   are then deleted again; the prototype stays in git history.

Each stage ends with `/code-review` by a reviewer that did not write it,
then one full `pwsh ./build.ps1`. Commits wait for the owner.
