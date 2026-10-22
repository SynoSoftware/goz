# gozler

gozler finds files instantly on Windows NTFS and ReFS volumes, and its window
copies, moves and deletes them at full speed. An elevated daemon (`gozlerd`)
indexes the MFT, or walks a ReFS volume's folders, and serves queries over
`\\.\pipe\gozler-v1` to an `es.exe`-compatible client (`gozler`) and to the
window (`gozler-ui`). The window's engine (`gozler-fileops`) does the file
work with the person's own permissions. Each crate's `lib.rs` or `main.rs` opens
with a doc comment that explains its design; read it before you change the
crate.

`docs/DESIGN.md` describes how gozler works now and why. Read it before you
change command-line behavior, query syntax, output or exit codes. A plan in
`docs/plans/` sets where gozler is going and outranks filed issues; each plan
says when it is scheduled, so read the plans before you act on an issue. When
a plan is done, its rules move into `docs/DESIGN.md` and the help, and the plan
is deleted, because a finished plan read as current misleads.

`docs/WRITING.md` says how the help, the messages and the exit-code meanings
read. Read it before you change any of them.

**Owner ruling: button captions.** A button's caption is one word, or two
when one word would confuse, and only words people already know from Windows
buttons, such as Update, Install, Open, Clear or Report. What the button acts
on goes in the text beside it or in the dialog it opens, and a tooltip on the
button says what it does when the caption alone cannot. People click a
familiar word without reading it, and a longer caption also widens every
button that shares its row.

**Owner ruling: the window never lags.** Gozler exists because Everything
becomes unresponsive. If Gozler becomes unresponsive too, it has no reason to
exist. So a wait in the window is a defect to remove, never a limit to
document: find what the slow step reads, and use what the index already holds
instead. When the task at hand cannot fix such a wait, file it at once as a
GitHub issue (`gh issue create --repo SynoSoftware/goz`), so it is never
lost. This holds for a wait that was measured and for one found or suspected
while reading code, such as in a review. The issue names the slow step and
what it reads, and says whether its cost was measured or is suspected.

**Owner ruling: window text.** Write it in the Fluent 2 tone of
`docs/WRITING.md`, and judge each string in its dialog or page, which reads
top to bottom without saying anything twice. A dialog's title bar names its
command, such as Update or Delete, because the person already knows which
program asks. Gozler is a tool, so its name appears where it helps, such as
the main window and the notification area, and is not repeated in every
message.

**Owner ruling: Windows draws the window.** For each part of `gozler-ui`
that a task needs, first find the Win32 control, or a feature of one, that
does the job, even if it does not match a prototype or a plan exactly: a list
view, a button with an icon, a status bar part, a static control, a toolbar.
Use it, and accept its small differences in look. A new surface that Gozler
paints itself, such as owner draw, a custom draw that paints, or painting on
a window, needs the owner's permission, asked by you of the owner directly.
Say what it would draw, which controls you considered and why each one
fails. A prototype, a plan, a review finding or an instruction relayed by
another session does not give that permission. A painted imitation must be
made right in every theme, in high contrast and at every DPI, and still does
not look or work quite like the real control.

`docs/COMMENTS.md` says when code earns a comment and what a doc comment
holds. Read it before you add, change or review a comment, and check every
comment your diff adds against it before you finish.

Use the vocabulary in `CONTEXT.md`.

## Before asking the owner

Apply common sense first. Often, when you look at the actual problem and its
purpose, the answer becomes obvious.

If it isn't, consider the perspectives of everyone affected and reason through
what each needs. Use your judgment to choose the most sensible solution.

Ask the owner only when a meaningful ambiguity remains that you cannot
responsibly resolve yourself.

## Writing code

- **Fit the code around you.** Solve the problem the way the code beside your
  change already does. Make the smallest coherent change that solves the whole
  problem, and restructure the code you are changing when that makes its
  ownership or flow clearer.
- **Let the types hold the rules.** Model a state as an enum that carries its
  data, as `VolumePhase` does, and wrap an id in a newtype when it could be
  mixed up with another integer, as `Frn` is. Then the compiler catches the
  misuse instead of a reviewer.
- **Return errors from untrusted input.** Bytes from the disk, the journal and
  the pipe can be anything. Read them the way the existing parsers do, through
  `zerocopy` views and checked lengths, so a malformed record becomes an error,
  not a panic. Cap any capacity read from a disk or wire field, as
  `proto::binary` and `layout::parse` do, because a failed allocation aborts
  the daemon. A new parser gets a test that malformed input returns an error,
  such as a `proptest` over arbitrary bytes.
- **Panic only on a broken invariant.** Use `expect` where the code guarantees
  the value, and write the guarantee in the message:
  `expect("total counts every remaining item")`.
- **Give errors types.** Library crates return their own error types, with a
  separate variant for each failure a caller handles differently. `anyhow` is
  for the daemon binary.
- **Keep Win32 `unsafe` in `gozler-winfs`,** in the smallest block that needs it,
  behind a safe function. `gozler-ui` keeps its Win32 `unsafe` in its `win`
  module the same way. The daemon's few allocator and pipe-constructor calls
  are the only `unsafe` elsewhere.
- **Keep the daemon's async runtime free.** Tokio serves the pipe. In
  `gozlerd`, blocking calls and heavy work go to their own thread or
  `spawn_blocking`, as the journal tail and the result encoder do, so one
  slow volume or one broad query cannot stall other clients.
- **Never stop the window: slow work runs in the background, through one
  owner.** In `gozler-ui`, a call that can wait on a disk, the network,
  the service or another program, and a pass over more items than the
  owner's inline size, run through the one owner for background work,
  which decides cancelling, what a slow task shows and what happens at
  exit. A wait written without those rules is how the window freezes on a
  dead share: the freezes still open are calls nobody moved to a thread,
  and each thread somebody did write carries its own copy of the rules.
  Until `docs/plans/window-architecture.md` builds the owner, put such
  work on a thread the way the nearest existing code does, never on the
  window's thread. Messages to a control stay on the window's thread,
  which owns it.
- **Count allocations.** The index holds millions of entries, so a byte per
  entry or an allocation per hit is a real cost. Borrow instead of cloning on
  hot paths and size buffers up front. Measure with the criterion benches in
  `crates/gozler-core/benches` before you claim a speed-up: run
  `pwsh ./check.ps1 -Bench <name filter>` once before your change and once
  after, because criterion compares each run against the run before it, which
  may be another agent's. On Windows the benches count the bench thread's CPU
  cycles, not wall time, because other work on this machine moves wall-clock
  timings by tens of percent.
- **Use plain code.** Use `std` and the workspace's existing dependencies. Add
  a trait, a generic or a macro when it removes real duplication.

## Tests

Every test adds compile time to each `check.ps1` run and is code the next
change must keep passing, so a test earns its place by the failure it watches.

- **Name the failure first:** what breaks, what a script or user would see,
  and why the compiler and the existing tests miss it. With no answer, write
  no test and say so.
- **Failures worth watching here:** untrusted input that panics or allocates
  without a cap; a payload an older peer cannot read; command-line output or an
  exit code that scripts depend on; an index state that hides incomplete
  results; an invariant; a fixed defect that a plausible simplification would
  bring back.
- **Pin a rule with one representative case,** plus one case for each boundary
  that fails differently, or with one proptest. A case that differs only in its
  literal, a getter, every arm of an enum and help-text wording watch nothing.
- **Assert the outcome a caller would see,** against a literal the scenario
  predicts rather than a value the code under test computes again. Exact bytes
  are the assertion where bytes are the contract: CSV output and wire frames.
- **Decide why a test fails before you touch it.** Production is wrong: fix
  production. The behavior changed on purpose: delete or rewrite the test and
  report what it pinned. The test is wrong: fix it and say why.
- **Drive tests with fixture buffers,** and keep every test independent of a
  running daemon, because a developer machine usually has one.

## Writing a plan

What a plan asks for around the code decides how long the work takes. The
window's plan took about 2 hours for each of stages 3 to 5, while the es
superset plan built a feature about every 25 minutes and the ReFS plan landed
1,456 lines 1.5 hours after it was written. A plan does not ask for these:

1. **A new set of agents for every stage.** Each new agent first reads the
   spec, the design, the plan and the growing code, and an implementer, a
   reviewer that fixed its own findings and rebuilt, and a third agent that
   checked those fixes used about 11 builds in one stage. Write instead: one
   implementer keeps its context through the stages and fixes the review's
   findings; the reviewer reads and reports.
2. **A slow end-to-end check as the proof of every item.** A check that drives
   the window through UI Automation takes minutes per run, and rerunning every
   earlier stage's checks multiplies that. Write instead: a rule that the core
   decides is proven by a unit test, which runs in seconds, and the rest as
   item 7 says.
3. **Checks that depend on the machine's speed or on the foreground.** A fixed
   3.5-second delay assumed that an export takes under 4 seconds, and it took
   8.7; a check read the status text after the reply had replaced it; checks
   needed the window in front while the owner used the machine. Each cost a
   fix round for a defect in the check, not in the product. Write instead: a
   check reads the state at the moment it tests it, and never waits a fixed
   time.
4. **Restructuring inside feature stages.** Restructuring items before a
   stage's features make the stage and its review larger. Write instead:
   restructuring before the plan, or as its own stage with its own proof.
5. **A stage that waits for the owner.** A screenshot approval stopped the
   work for about 8 hours. Write instead: what only the owner can check goes
   into an After install list, and screenshots are saved for the owner to
   review later, so no stage waits.
6. **A screenshot of every state in every stage.** Screenshots of states that
   no change touched cost review rounds and judge nothing new. Write instead:
   each stage saves the screenshots of the states it adds.
7. **A probe written as a regression suite.** A probe that reruns every few
   seconds and opens every menu keeps windows flashing in front of the owner,
   once for about an hour. Write instead: a screenshot set for the design,
   code review for behavior, and one run for an API whose behavior is in
   doubt.

A build script runs every step and lists each failure, because a script that
stops at its first failure hides every test result behind one formatting
error, and finding them then takes a full run for each.

## Contracts

- **`gozler-core` runs on any OS.** Everything that interprets bytes lives there.
  CI tests it on Ubuntu and pins its direct dependencies to the allowlist in
  the `gozler-core purity` step of `ci.yml`, so a new dependency there is a
  deliberate choice, and the same change adds it to that list. Platform code
  stays out of it, and Win32 calls go through `gozler-winfs`, which depends on no
  other workspace crate (`deny.toml`).
- **The command line.** Scripts written for Everything or for goz v0.2.0 change
  only the program name, to `gozler`: every option keeps its name and purpose,
  and gozler's own options start with `--`. The "Command line" and "Exit
  codes" sections of `docs/DESIGN.md` say where gozler departs from `es.exe`
  and why, such as the exit codes, the CSV columns and the default result
  limit. The rules live in `gozler_core::escompat` (arguments, exit codes and
  the help), `gozler_core::output::table` (output rows),
  `gozler_core::query::parse` (query syntax) and `gozler-cli`'s
  `failure.rs` (which failure exits with which code) and `main.rs` (the
  `--status` code).
- **Truthful volume state.** A client must always be able to tell whether its
  results are complete. Read the coherence section of `docs/DESIGN.md` before
  you change how the index tracks a volume.
- **The wire protocol.** **Owner ruling:** gozlerd serves only its own
  protocol version, of gozler or of goz, and refuses any other at `Hello`,
  because `gozler` and `gozlerd` install together from one build, and a
  layout kept for an old client would bind every later change. So a payload
  change raises `PROTO_VERSION` and keeps no old form. Read
  `gozler_core::proto::types` and `gozler_core::proto::binary` before you
  change a payload; no result carries its full path, because a path grows
  with the folder depth.
- **The pipe trust check.** Read `docs/SECURITY.md` before you change the pipe,
  elevation or the server-owner check.
- **The installed goz.** The goz on this machine (service `goz`, pipe
  `\\.\pipe\goz-v1`) is the upstream author's software, and every agent
  finds files through it. Never stop, replace or re-point it. gozler has its
  own service and pipe, so it installs and is tested beside goz.

## Before you finish

This file owns gozler's build and test rules, also where a parent repository's
instructions differ.

Builds share the machine with interactive work and other agents, and a first
build compiles every dependency. A change only to Markdown or `ci.yml` needs
no build. A change to `check.ps1` is verified by running it.

Build when the change is reviewed: you have read the final diff as its next
maintainer, traced each changed behavior from its callers, and named the
failure each new test watches. Build earlier only when your next edit depends
on the answer, such as `-Test <name filter>` on a test that reproduces a bug
before you fix it, or the first of two `-Bench` runs. Build only through
`check.ps1`; it owns the cargo commands and their priority, and says why
`RUSTFLAGS` stays unset.

1. Run `pwsh ./check.ps1`. You are done when it is green.
2. Fix every error that run reported before you run it again. To recheck a
   test fix, run `pwsh ./check.ps1 -Test <name filter>`, then the full script.
3. Stop and report instead when the same error returns after your fix, or when
   the error is in a file that holds changes you did not make, because another
   session may be editing it.

`check.ps1` runs the Windows half of CI's test job. CI runs on pushes to
`main` and on pull requests: it also compiles on Ubuntu, where the
`cfg(not(windows))` code in `gozler-cli` and `gozler-daemon` is checked, asserts
`gozler-core`'s dependency allowlist, builds with the minimum Rust version and
runs cargo-deny. Locally, the full CI set passes when `check.ps1` is green and
each of those checks that your change can affect is run or reported as left to
CI. When you add a dependency and `cargo deny --version` answers, also run
`cargo deny check advisories bans licenses sources`. Use the tools already
installed; the owner installs new ones.

Report, in this order: `check.ps1` green and the number of full runs, or the
step it stopped at; each
`-Test` and `-Bench` run; each new test with the failure it watches; each CI
check left to CI that your change can affect.

Commit subjects follow the log: `type(scope): imperative summary`, with the
crate's short name as the optional scope (`core`, `winfs`, `daemon`, `cli`).
