# gozler

gozler finds files instantly on Windows NTFS and ReFS volumes, and its window
copies, moves and deletes them at full speed. An elevated daemon (`gozlerd`)
indexes the MFT, or walks a ReFS volume's folders, and serves queries over
`\\.\pipe\gozler-v1` to an `es.exe`-compatible client (`gozler`) and to the
window (`gozler-ui`). The window's engine (`gozler-fileops`) does the file
work with the person's own permissions. Each crate's `lib.rs` or `main.rs`
opens with a doc comment that explains its design; read it before you change
the crate.

## Read for the change

- `docs/DESIGN.md` describes how gozler works now and why. Read it before you
  change command-line behavior, query syntax, output or exit codes.
- `docs/plans/` sets where gozler is going and outranks filed issues. Each
  plan says when it is scheduled, so read the plans before you act on an
  issue. When a plan is done, its rules move into `docs/DESIGN.md` and the
  help, and the plan is deleted, because a finished plan read as current
  misleads.
- `docs/WRITING.md` says how the help, the messages, the exit-code meanings
  and the window's text read. Read it before you change any of them.
- `docs/COMMENTS.md` says when code earns a comment and what a doc comment
  holds. Read it before you add, change or review a comment, and check every
  comment your diff adds against it before you finish.
- `CONTEXT.md` holds the vocabulary. Use its words.

## Common sense before asking

Look at the actual problem and its purpose; often the answer is then obvious.
If it is not, reason through what each person affected needs, and choose the
most sensible solution. Ask the owner only when a meaningful ambiguity remains
that you cannot responsibly resolve yourself.

## The window

**Owner ruling: established behavior first.** Search behaves as Everything
does, and Commander mode as Total Commander and Norton Commander do. Where they
say nothing, the window behaves as File Explorer does and as Nielsen's
usability heuristics and Microsoft's Windows design guidelines say. People
already know that behavior; a different design names what the person
gains. For example, a command that takes the person to another mode, folder
or dialog first checks that the destination opens; when it does not, the
person stays where they were and is told why.

**Owner ruling: the window never lags.** Gozler exists because Everything
becomes unresponsive. If Gozler becomes unresponsive too, it has no reason to
exist. So a wait in the window is a defect to remove, never a limit to
document: find what the slow step reads, and use what the index already holds
instead. When the task at hand cannot fix such a wait, file it at once as a
GitHub issue (`gh issue create --repo SynoSoftware/goz`), so it is never
lost. This holds for a wait that was measured and for one found or suspected
while reading code, such as in a review. The issue names the slow step and
what it reads, and says whether its cost was measured or is suspected.

In code, slow work runs in the background through one owner. A call that can
wait on a disk, the network, the service or another program, and a pass over
more items than the owner's inline size, run through `core::work`,
`win::work` and `app::work`, which decide cancelling, what a slow task shows
and what happens at exit. Read "The window" in `docs/DESIGN.md` before you
add such work. A thread written beside that owner carries its own copy of
those rules, and a call left on the window's thread is how the window freezes
on a dead share. Domain engines keep their own workers, scheduling and
recovery. Messages to a control stay on the window's thread, which owns it.

**Owner ruling: Windows draws the window.** For each part of `gozler-ui`
that a task needs, first find the Win32 control, or a feature of one, that
does the job, even if it does not match a prototype or a plan exactly: a list
view, a button with an icon, a status bar part, a static control, a toolbar.
Use it with Windows' default behavior and look, and accept its small
differences from the design. A look the owner or a design wants, such as
another selection color or no border around a row, is reached with the
control's own styles and settings first, as the list's selection drops the
"Explorer" visual style for its border.

A new surface that Gozler paints itself, such as owner draw, a custom draw
that paints, or painting on a window, is only for a task that no native
control can do at all, or for the part of a look the owner asks for that no
setting reaches. It needs the owner's permission, asked by you of the owner
directly: say what it would draw, which controls you considered and why each
one fails. A prototype, a plan, a review finding or an instruction relayed by
another session does not give that permission. The test `win::painting`
fails the build on any painting outside the functions its `allowed` lists,
and only the owner's permission adds one. A painted imitation must be made
right in every theme, in high contrast and at every DPI, and still does not
look or work quite like the real control.

**Owner ruling: window text.** Write it in the Fluent 2 tone of
`docs/WRITING.md`, and judge each string in its dialog or page, which reads
top to bottom without saying anything twice. A dialog's title bar names its
command, such as Update or Delete, because the person already knows which
program asks. Gozler is a tool, so its name appears where it helps, such as
the main window and the notification area, and is not repeated in every
message. A page shows what the person works with. Why something will not
work, and how it works inside, appear where the person meets them: in a
tooltip, or in a message when the person picks the value they concern.
A note shown just in case crowds the page, and people skip it. Such a
message builds the keys, values and limits it names from the code that
owns them, so a later change to that code cannot leave it stale.

**Owner ruling: button captions.** A button's caption is one word, or two
when one word would confuse, and only words people already know from Windows
buttons, such as Update, Install, Open, Clear or Report. What the button acts
on goes in the text beside it or in the dialog it opens, and a tooltip on the
button says what it does when the caption alone cannot. People click a
familiar word without reading it, and a longer caption also widens every
button that shares its row.

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
- **Keep Win32 `unsafe` in `gozler-winfs`,** in the smallest block that needs
  it, behind a safe function. `gozler-ui` keeps its Win32 `unsafe` in its
  `win` module the same way. The daemon's few allocator and pipe-constructor
  calls are the only `unsafe` elsewhere.
- **Keep the daemon's async runtime free.** Tokio serves the pipe. In
  `gozlerd`, blocking calls and heavy work go to their own thread or
  `spawn_blocking`, as the journal tail and the result encoder do, so one
  slow volume or one broad query cannot stall other clients.
- **Count allocations.** The index holds millions of entries, so a byte per
  entry or an allocation per hit is a real cost. Borrow instead of cloning on
  hot paths and size buffers up front. Measure with the criterion benches in
  `crates/gozler-core/benches` before you claim a speed-up. Compare matched
  before/after runs of `pwsh ./build.ps1 -Bench <name filter>`, because
  criterion's previous run may belong to another agent. Reuse a valid
  baseline; a bench run is a build, so it takes the steps in "Building and
  finishing". On Windows the benches count the bench thread's CPU cycles, not
  wall time, because other work on this machine moves wall-clock timings by
  tens of percent.
- **Use plain code.** Use `std` and the workspace's existing dependencies. Add
  a trait, a generic or a macro when it removes real duplication.

## Tests

Every test adds compile time to each `build.ps1` run and is code the next
change must keep passing, so a test earns its place by the failure it watches.
Write a test, take a capture or run a check only to answer an open question
that the code, the compiler and the platform cannot settle, and write that
question down before you run it. An explicit request from the owner is a
question of its own. Reuse existing evidence that already answers it.

- **Name the failure first:** what breaks, what a script or user would see,
  and why the compiler and the existing tests miss it. With no answer, write
  no test and say so.
- **Failures worth watching here:** untrusted input that panics or allocates
  without a cap; a payload an older peer cannot read; command-line output or an
  exit code that scripts depend on; an index state that hides incomplete
  results; an invariant; a fixed defect that a plausible simplification would
  bring back.
- **Pin a rule with one representative case,** plus one case for each boundary
  that fails differently, or with one proptest. A regression test watches the
  reported failure and its boundaries, not adjacent features. A case that
  differs only in its literal, a getter, every arm of an enum and help-text
  wording watch nothing.
- **Assert the outcome a caller would see,** against a literal the scenario
  predicts rather than a value the code under test computes again. Exact bytes
  are the assertion where bytes are the contract: CSV output and wire frames.
- **Decide why a test fails before you touch it.** Production is wrong: fix
  production. The behavior changed on purpose: delete or rewrite the test and
  report what it pinned. The test is wrong: fix it and say why. Keep the
  behavior the test checks; a weaker assertion that turns green hides the
  failure.
- **Drive tests with fixture buffers,** and keep every test independent of a
  running daemon, because a developer machine usually has one.
- **Test what lasts.** Test lasting behavior and invariants once the
  implementation settles, and prune redundant tests. Code the current work
  will replace or delete gets no test.

**Keep diagnosis proportional.** Trace the failing behavior and choose the
smallest credible check. Reuse existing tests and fixtures before building
new infrastructure. If a new probe or fixture fails before it reaches the
behavior under investigation, correct its setup once; if it still cannot
reach that behavior, check it a simpler way instead of extending the harness.
Once the cause is established, implement the correction, and reopen the
investigation only when new evidence contradicts it. When you report a
failure or a delay, say whether it is a product defect, a mistake in a newly
written check or an environment restriction.

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

## Contracts

- **`gozler-core` runs on any OS.** Everything that interprets bytes lives
  there. CI tests it on Ubuntu and pins its direct dependencies to the
  allowlist in the `gozler-core purity` step of `ci.yml`, so a new dependency
  there is a deliberate choice, and the same change adds it to that list.
  Platform code stays out of it, and Win32 calls go through `gozler-winfs`,
  which depends on no other workspace crate (`deny.toml`).
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

## Building and finishing

This file owns gozler's build and test rules, also where a parent repository's
instructions differ.

A build takes minutes of the owner's shared laptop, which interactive work and
other agents also use, and a first build compiles every dependency. Build only
through `build.ps1`; it owns the cargo commands and their priority, and says
why `RUSTFLAGS` stays unset. It runs every step and lists each failure,
because a script that stops at its first failure hides every test result
behind one formatting error, and finding them then takes a full run for each.
A change only to Markdown or `ci.yml` needs no build. A change to `build.ps1`
is verified by running it.

**Owner preference: use judgment, not ceremony.** Code larger coherent
batches: finish the agreed change, replace or delete its temporary paths, and
review it before you compile. Reuse valid review and verification evidence.

**Owner ruling: compile at the end, and only for a reason that survives a
challenge.** The owner stops a build whose reason they cannot see. During
development, read the code and use existing binaries. This covers every
compile: `build.ps1` in any mode, tests, benchmarks, probes, experiments and
release packages. Take these steps in order, and stop at the first that
fails.

1. **Need.** Try to answer the question without a build. Read the code, the
   compiler's guarantees, documented Windows behavior, and the record:
   `docs/plans/`, `docs/research/`, `docs/DESIGN.md` and the open issues. A
   mechanism a plan lists as rejected already has its answer, and so does a
   narrower form of it, such as the same window style turned on for one
   frame. Reassurance, a small edit, a harness change and a skill's test
   loop are not a need.
2. **Justify.** In the message that will start the build, write the one
   question it answers, what the record says about it, two results the
   build can give with the different next edit each one leads to, and that
   every edit a build can answer is in the tree, so one build answers them
   all.
3. **Challenge.** Try to refute the justification: find the answer in the
   code or the record, predict the result, or show that both results lead
   to the same edit. Where the host can launch one, give the justification
   to a fresh agent that did not write it, because an author approves its
   own reasons. A justification that loses any part stops the build.
4. **Nobody built it.** Check that no other session has built your code, as
   the next ruling says.

Then run the smallest check that answers the question, such as
`-Test <name filter>`, and return to development. An explicit request from
the owner, such as an exe to try, and the final verification below skip
steps 1 to 3 and still take step 4. A build the owner stops stays
stopped: start it again only with a new reason that passes these steps,
and say what changed.

**Owner ruling: never compile what another session already compiled.** All
sessions work in this one checkout, so a build by any session compiles every
session's uncommitted changes, yours included. Before you compile, find out
whether another session has built the tree since your last edit, or is
building it now: list the running `build.ps1` and `cargo` processes, and ask
the sessions at work. If it has, do not compile, do not wait so that you can
compile after it, and do not run a "focused" check instead. Ask that session
for its result and report what it proved. The same code compiled twice gives
the same result, so a second build only takes minutes of the owner's shared
laptop.

Final verification starts when you have read the final diff as its next
maintainer, traced each changed behavior from its callers, and named the
failure each new test watches.

1. Run `pwsh ./build.ps1`, unless another session's build already covers
   your change.
2. Fix all reported errors before rerunning, and repeat until the full script
   is green. A focused `-Test <name filter>` may verify a correction before
   the final full run.
3. If a failure belongs to another session's work outside the authorized task,
   report it instead of changing those files without authorization.

`build.ps1` runs the Windows half of CI's test job. CI runs on pushes to
`main` and on pull requests: it also compiles on Ubuntu, where the
`cfg(not(windows))` code in `gozler-cli` and `gozler-daemon` is checked,
asserts `gozler-core`'s dependency allowlist, builds with the minimum Rust
version and runs cargo-deny. Locally, the full CI set passes when `build.ps1`
is green and each of those checks that your change can affect is run or
reported as left to CI. When you add a dependency and `cargo deny --version`
answers, also run `cargo deny check advisories bans licenses sources`. Use the
tools already installed; the owner installs new ones.

Report, in this order: `build.ps1` green and the number of full runs, or the
step it stopped at; each `-Test` and `-Bench` run; each new test with the
failure it watches; each CI check left to CI that your change can affect.

Commit subjects follow the log: `type(scope): imperative summary`, with the
crate's short name as the optional scope (`core`, `winfs`, `daemon`, `cli`).
