# Service repair and the status bar

Status: proposed. Scheduled after the Settings search and Clear buttons are
committed, and before the Include page's window stage, because both change
Settings > Drives and service.

## What the person sees today, and why it fails

When the service cannot search, the list and Settings > Drives and service
show a sentence. Two cases leave the person with work that the window
should do:

- "Download Gozler and run its setup" opens a web page. The person must
  find the right file, download it and run it.
- "For the full report, run gozler --status" asks the person to open a
  terminal.

The fix buttons that do run (Start, Turn on, Install and Update the service)
start an elevated `cmd /c` and forget it. When Windows' permission prompt is
refused, or a command fails, nothing says so; the window only notices when
the service answers.

## The result

1. Each failure that has a fix shows one button, in the list's message and
   at the bottom right of the Service group on Drives and service. The
   button names the action: "Start the service", "Turn on the service",
   "Install the service", "Update the service", "Download and install
   Gozler", "Open Services".
2. The Service group has a second button, "Show report", always enabled
   while the service answers. It opens a dialog with the text that
   `gozler --status` prints, in a read-only box, with Copy and Close. The
   sentence about `gozler --status` goes.
3. A running fix shows in the status bar, with its step, its progress and,
   while it can stop, a Cancel button. The fix's result stays there until
   the next fix starts, as an export's result does today.
4. The Drives list says why it is empty: "Gozler cannot read the drives
   while the service does not answer." in the list's empty area.

## The service fixer, one component

`core::fixer` owns what a fix is and where it is; `app::fixer` runs it. The
main window owns the one fixer, because a fix outlives the Settings dialog.
Both the list's button and the Service group's button call the same
`Fixer::start`, so the two can never run different code.

A fix moves through steps. Each step has its own text, its progress kind
and whether Cancel works in it:

| Step | Status bar text | Progress | Cancel |
| --- | --- | --- | --- |
| Asking Windows | "Waiting for permission…" | none | no: the prompt is Windows' |
| Running the commands | "Starting the service…", "Installing the service…", "Updating the service…" | indeterminate | no: a half-run `sc` command is worse than a finished one |
| Waiting for the service | "Waiting for the service to answer…" | indeterminate | yes |
| Finding the release | "Finding the latest Gozler…" | indeterminate | yes |
| Downloading | "Downloading Gozler 0.4.0… 3.1 of 8.4 MB" | bytes of total | yes |
| Checking the file | "Checking the download…" | indeterminate | no: it takes under a second |
| Starting the setup | "Gozler closes to install 0.4.0." | none | no |

The result is one of: done ("The service runs again."), cancelled ("Update
cancelled."), refused ("Windows did not give permission."), or failed, with
the reason ("The service could not be started: <Windows' reason>.").

- **Elevated commands are watched.** `ShellExecuteExW` with
  `SEE_MASK_NOCLOSEPROCESS` gives the process; a worker thread waits for
  it and reads its exit code. `ERROR_CANCELLED` from the prompt is the
  refused result. After the commands, the fixer waits up to 30 seconds for
  the service to answer, through the status poll that already runs.
- **Download and install** asks GitHub for the latest release, which the
  update check already does, takes the asset named
  `gozler-<version>-x64-Setup.exe`, downloads it to
  `%TEMP%\Gozler\` and compares its SHA-256 with the asset's `digest` in the
  reply. A file that differs is deleted and the fix fails. Then the window
  starts the setup and exits, because the setup refuses to run while a
  Gozler window holds its mutex. The setup's own window shows the install
  and offers to start Gozler again.
- **A release that is not newer than the service** ends the fix with "No
  newer Gozler is published. Install the service from the same build as
  this window." Downloading it would not help.
- **Cancel** sets the fix's token; the download checks it between reads,
  deletes its partial file and ends as cancelled. One fix runs at a time:
  while one runs, every fix button is disabled.

## The status bar, one component

The main window's four parts become three groups, as TinyTorrent2's status
bar has them: what the view holds on the left, work in the middle, and the
service on the right. `app::status_bar` owns the Win32 status bar, its
parts and the child controls in them; `core::captions` gives it the texts.

| Group | Shows |
| --- | --- |
| Left | today's parts 1 and 2: "1,204 results · 3 selected · 12.4 MB" |
| Middle | each running job, the export and the fix, with its text, a progress bar when it has progress, and Cancel; or the last job's result |
| Right | today's part 4: the index state and "Gozler 0.4.0 is available" |

- The middle group's Cancel replaces File > Cancel export as the only way to
  stop an export from the status bar; the menu item stays.
- The Settings dialog gets the same status bar at its bottom, below OK,
  Cancel and Apply. Its left group says "Settings · Drives and service",
  its middle group shows the same jobs as the main window's, and its right
  group the service state. While a fix runs, the person who started it from
  Settings sees it without closing the dialog.

## What stays

- The commands each fix runs are `gozler`'s, as today, so the terminal and
  the window repair the same way.
- No fix runs by itself; each starts from a click.
- The update check stays a daily request; Download and install makes the
  second kind of request, and only on a click.

## Stages

1. The status bar component, with today's texts in three groups, and the
   export in the middle group with its Cancel. Proof: screenshots of each
   state, and the existing layout tests extended to the three groups.
2. The fixer: watched elevated commands, the steps and results, the buttons
   in the list and in the Service group, the empty drive list text.
   Proof: a unit test of the step machine (each step's cancel rule, each
   result), and one run of a refused permission prompt.
3. Download and install, with the digest check. Proof: a unit test that a
   wrong digest fails and deletes the file, and one real download of the
   latest release.
4. Show report. Proof: a screenshot.
