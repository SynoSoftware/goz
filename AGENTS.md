# gozler

gozler is an instant filename search for Windows NTFS: an elevated daemon
(`gozlerd`) indexes the MFT and serves queries over `\\.\pipe\goz-v1` to an
`es.exe`-compatible client (`gozler`). Each crate's `lib.rs` or `main.rs` opens
with a doc comment that explains its design; read it before you change the
crate.

The plans in `docs/plans/` set where gozler is going and outrank filed issues;
each plan says when it is scheduled. Read them before you change command-line
behavior, query syntax, output or exit codes, or before you act on an issue.

Use the vocabulary in `CONTEXT.md`.

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
  behind a safe function. The daemon's few allocator and pipe-constructor calls
  are the only `unsafe` elsewhere.
- **Keep the async runtime free.** Tokio serves the pipe. Blocking calls and
  heavy work go to their own thread or `spawn_blocking`, as the journal tail and
  the result encoder do, so one slow volume or one broad query cannot stall
  other clients.
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
- **Link code in doc comments** with intra-doc links (``[`Frn`]``), so the CI
  doc build fails when a name changes.

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

## Contracts

- **`gozler-core` runs on any OS.** Everything that interprets bytes lives there.
  CI tests it on Ubuntu and pins its direct dependencies to the allowlist in
  the `gozler-core purity` step of `ci.yml`, so a new dependency there is a
  deliberate choice, and the same change adds it to that list. Platform code
  stays out of it, and Win32 calls go through `gozler-winfs`, which depends on no
  other workspace crate (`deny.toml`).
- **The command line.** Scripts written for Everything or for goz v0.2.0 change
  only the program name, to `gozler`: every switch keeps its name and purpose,
  and gozler's own options start with `--`. `docs/plans/es-superset-specs.md`
  decides where gozler departs from `es.exe`, such as the exit codes, the CSV
  columns and the default result limit. The rules live in
  `gozler_core::escompat` (arguments), `gozler_core::output::table` (output rows),
  `gozler_core::query::parse` (query syntax) and `gozler-cli`'s `main.rs` (runtime
  exit codes).
- **Truthful volume state.** A client must always be able to tell whether its
  results are complete. Read the coherence section of `docs/DESIGN.md` before
  you change how the index tracks a volume.
- **The wire protocol.** An installed daemon and a newer client must still
  talk. Read `gozler_core::proto::types` and `gozler_core::proto::binary` before you
  change a payload. Results pages are positional binary, so any change to the
  item layout breaks older clients. A new JSON field gets `#[serde(default)]`,
  so messages from older peers still parse, and its default means what an
  older peer's silence means.
- **The pipe trust check.** Read `docs/SECURITY.md` before you change the pipe,
  elevation or the server-owner check.
- **The running daemon.** The daemon installed on this machine owns
  `\\.\pipe\goz-v1`, and other tools search through it. Ask the owner before
  you stop, replace or reinstall it.

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
