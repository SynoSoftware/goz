# Paths through links

Status: implementation independently reviewed, with all findings resolved.
Focused verification and release packaging passed. Nothing is committed.

## Execution record

One implementation owner edits this checkout. The other active link-path
chat is read-only and has started no build; its review record is
`V:\temp\goz-paths-through-links-review.md`.
The independent grouped-payload delta verdict is clean. It traced matching,
scope/coverage, indexed target spelling, stale worker replies and retirement.

The current batch replaces path strings with finite literal and wildcard
states, shares nearest-target ancestry with `DirPaths`, and applies negation
after combining a term's paths. Graph construction, normalization and bold
target lookup use indexed prefixes. Bold compilation runs in the existing
window work owner and rejects replies for another search or link table.
The retained sources now check tags before reading reparse data, honor its
declared bounds, refresh on reparse changes, and discard deleted targets.
The table's fingerprint is computed on construction/deserialization, off UI.

Round 1 corrections use `Links::search_scope` for daemon routing, CLI timeout
coverage and the window's per-list cached scope. `Scope::includes` shares
descendant primary-mount routing between scans and coverage. Targets are
canonicalized from the destination index before graph compilation or delivery
to the window, so case-sensitive descent reads indexed spelling. Volume GUID
targets route through mount metadata too. The internal scope clause is added
after validating user terms, without consuming their last slot.
The same per-list worker resolves ordinary panel folders; folder-size status
and the unindexed-folder message use that cache, guarded by folder and table.

New checks watch silent differences between state stepping and whole-path
matching on random cyclic graphs, loop/nested-target remainders, visible bold
text, reparse framing, independent term negation, stale bold replies,
target-volume indexing completion, primary folder mounts, target spelling and
the user-term boundary through links, and a healthy source volume hiding a
folder panel's offline, metadata-pending or unindexed link target.
Existing path-coherence and wire round-trip checks cover the changed owners,
but their names were not selected by the focused filter.

The focused build question was: do the compiled finite-state evaluator and its
window/protocol integration satisfy those fixtures? Before the run the record
contained no compile or test result for this implementation. Source review
cannot prove agreement over the generated Unicode/wildcard graphs. A failing
fixture requires correcting the transition/integration it identifies; a passing
run leaves the release build as the next step. Every edit this check could
answer was in the tree. The independent reviewer accepted the challenge.
`build.ps1 -Test links` passed on its first run: 15 core, one daemon and
three window tests, 19 total, with no failures. Compilation took 3m25s.
Process inventories and coordination found no other build covering it.
No duplicate build is permitted. The full build remains deferred until the
owner requests a commit, as ruled below. No full run or benchmark was started.

`build.ps1 -Release` passed on its first run in 3m58s, after a fresh process
inventory and coordination confirmed no other package covered the source.
The executables are `target/release/gozler.exe`, `target/release/gozlerd.exe`
and `target/release/gozler-ui.exe`. No application or service was installed or
started. The installed upstream goz was not touched. Other sessions' window
changes remain uncommitted alongside this feature.

| Release file | SHA-256 |
|---|---|
| `gozler.exe` | `E3696890F0AFFC881305F804B44E997916BAECF53140A697881BC54593EF8868` |
| `gozlerd.exe` | `4DA48DDC5BF769DF2E41FF1838EC37731E9993BB43500B37CFA21BC711D0E5C4` |
| `gozler-ui.exe` | `EFA16B407B31D532C65973FAF59F2E59A6BD4E1CB17E88A4721F2AF9BC9D432D` |

The focused run includes these new or extended lasting checks:

| Test | Failure watched |
|---|---|
| `a_link_leads_to_a_folder_on_a_drive_or_nowhere` | Reparse targets resolve incorrectly, including volume GUID and relative targets. |
| `any_bytes_parse_without_panic` | Untrusted reparse bytes panic. |
| `a_reparse_name_stays_inside_its_declared_aligned_payload` | A name escapes its declared payload or uses unaligned UTF-16. |
| `a_path_through_links_is_its_real_folder` | Nested scope links resolve to the wrong folder. |
| `loops_and_nested_targets_preserve_remainders_through_links` | Loops or nearest nested targets lose a remainder, or accept a false word boundary. |
| `bold_marks_only_the_visible_remainder_through_links` | Hidden link-prefix text or wrong Unicode offsets mark the displayed path. |
| `states_agree_with_whole_paths_on_random_links` | Finite-state stepping silently differs from the existing whole-path matcher. |
| `a_search_through_links_finds_what_is_below_their_targets` | Independent positive/negated terms or linked scopes lose or add rows. |
| `queries_through_links_keep_the_user_term_limit` | Internal scope enforcement consumes a user term or bypasses the term limit. |
| `coverage_through_links_includes_targets_and_descendant_primary_mounts` | Coverage omits the target or a volume mounted below the scope. |
| `links_use_indexed_target_case_and_scopes_enter_primary_folder_mounts` | Reparse spelling breaks case-sensitive queries, or routing skips a primary folder mount. |
| `stale_bold_from_another_search_or_links_is_rejected` | A late worker reply marks a newer search or link table. |
| `folder_coverage_through_links_reports_the_target_instead_of_the_source` | A healthy source hides a folder panel's offline, metadata-pending or unindexed target. |
| `coverage_through_links_researches_when_the_target_finishes_indexing` | Target indexing completes without refreshing the linked search. |

Ubuntu compilation/tests, the minimum-Rust build and the dependency purity
gate remain to CI. Dependencies and the lockfile did not change. The deferred
full Windows run will check clippy, the unfiltered tests and rustdoc.

The requested matched before/after benchmarks remain unverified. Historical
Criterion estimates from October 18-26 have no attributable source revision
and fixture for this batch, and the earlier design A never compiled. They
are not a valid baseline. No speedup or measured absence of regression is
claimed; the source review establishes the finite-state and indexed-prefix
structure only. Placeholder hydration and rendered bold behavior also remain
unmeasured.

## Goal

**Owner ruling:** "If I can cd to it, it must find it." A path that a person
can open finds the same files that a search of the real folder finds. This
holds through junctions, folder symbolic links and folders that mount a volume,
with or without a drive, and at any depth.

Example: `C:\SynoSoftware\TinyTorrent2\artifacts` is a junction to
`V:\artifacts\TinyTorrent2`. Today `artifacts\evidence` finds nothing, because
the index holds those files only at `V:\artifacts\TinyTorrent2\evidence\...`.
Agents write paths relative to a repository, so this is the case the owner
meets most.

## Chosen design: what each path term still needs at each link target

Three independent designs were compared with the two earlier ones (A and B,
below). Two of them arrived at the same design, and the third agreed on the
query part. This is the design to build.

1. **Link table.** gozlerd keeps one table of the followed links on all its
   volumes: each link's file ID, and its target. A link on C: can point into
   V:, so the table covers all volumes.
2. **Compile time, for each path term.** A link boundary in a path is always a
   `\`. For each target T, the daemon works out what the term still needs
   after a path that reaches T. There are three outcomes:
   - Nothing: the term lies wholly inside that path. Then T and everything
     below it match.
   - A remainder: the real path below T must start with it. For
     `artifacts\evidence`, the path `...\TinyTorrent2\artifacts` ends with
     `artifacts`, so T = `V:\artifacts\TinyTorrent2` gets the remainder
     `\evidence`.
   - No match: the target is dropped for this term, and no folder below it
     pays anything.
3. **States, not strings.** A literal term's state is "the first k bytes have
   been read". A wildcard's state is a token position. The states that reach
   each target are computed as a fixed point over the links. The state sets are
   finite, so chains of links work, and loops end by themselves without a cap.
   So loops are followed, as the owner's rule says. This reverses the earlier
   decision not to follow loops, which was made only because following them
   then needed a mechanism of its own.
4. **Run time.** A term passes when the real path passes it, as today, or when
   the path below the nearest target that still needs something passes one of
   that target's remainders, as a start test. The per-folder memo of `DirPaths`
   keeps the nearest such target above each folder. Nothing is stored for each
   entry.
5. **Each term passes on its own path.** Each term asks "does some path to
   this entry match?", and a negated term asks "does no path match?". With
   that rule, `artifacts\evidence !V:\` excludes a row whose real path, the one
   on screen, is on V:.
6. **Scope, Location and `parent:`** become their one real folder through the
   table (`Links::real_path`). The client's disk-based rewrite
   (`gozler_client::follow_links`) is deleted, so no client touches the disk
   for a search.
7. **Bold.** The window gets the link table from the status reply, which
   needs a `PROTO_VERSION` bump with no old form kept. It sends the table only
   when its generation number has changed. The window computes the same
   remainders with the same gozler-core function, and bolds the remainder's
   match in the real path, for example `\evidence`. A match that lies wholly
   in the link's own path shows no bold, because that text is not on screen.

### Matchers

| Matcher | How it crosses a link |
|---|---|
| Substring | The term can split at any `\` in it, also at position 0. |
| Word start, whole word | The same splits. The word boundary before the split is checked against the reaching path at compile time. A whole word's end is checked at run time. |
| `startwith:` | The only split is at the end of the reaching path. |
| Wildcard | The pattern positions left after reading the reaching path. One step over the existing tokens; `query/wildcard.rs` gains about 40 lines. |
| Regular expression | Tests the real path only. An exact form needs the automaton state after the reaching path, from `regex-automata`, which would be a new direct dependency of gozler-core. DESIGN says so in one sentence. |
| A word without `\` | Tests the real path only. A word names no folder to open, and through Windows' compatibility junctions a word such as `Application Data` would otherwise match all of `AppData\Roaming`. |

### Link targets

- Read each link with `FSCTL_GET_REPARSE_POINT` by file ID, after a volume's
  build. This is in the tree now. It works the same on NTFS and ReFS.
- Read only junctions and folder symbolic links. Today `tail.rs` opens every
  folder with the reparse attribute, and that includes OneDrive placeholder
  folders.
- After the build, read a folder again only for the journal reason
  `USN_REASON_REPARSE_POINT_CHANGE`, not for every batch. A rename needs no
  read, because a link's path is rebuilt from the index for each query.
- Each extra mount of a volume is a link, taken from the mount manager's list
  (`volume.mounts`).
- Not chosen: taking the targets from the MFT read that the index already
  makes (`QUERY_FILE_LAYOUT_INCLUDE_STREAM_INFORMATION` and
  `..._FOR_REPARSE_ATTRIBUTE`). It would save the opens after a build. But it
  is documented only in `winioctl.h` comments, is unverified, is NTFS only,
  and the journal path needs the open anyway. Reconsider it only if the opens
  after a build are measured as slow.

### Tests

- A parser proptest: malformed reparse bytes return an error.
- A proptest that compares this design's result with whole-path matching over
  random link graphs, terms and matchers. The main risk is that the step
  function disagrees with `Matcher` on word boundaries, the whole-word end,
  case folding or `?` over code points. Such an error loses or adds results
  without any sign, and this test catches it.
- Before and after criterion benches: a name-only query, `-p kernel32`, and a
  path term with a separator.

## Why not A or B

- **A** (the earlier implementation): it tests every
  entry against every path that reaches it, and all path terms must pass on
  one path together. That set has no small bound. A stops at `MAX_REACHES` =
  4096, and `server.rs` then only logs a warning and sends the results as if
  they were complete, which breaks truthful results. Its `via` loop compares
  every link with every target on each query: about 2.5 × 10⁹ prefix tests
  with the tens of thousands of junctions in a pnpm tree.
- **B** (expand each term into OR alternatives at the targets): this is the
  chosen design written as strings, and as strings it has gaps:
  - it accepts a split inside a word, such as `rtifacts\evidence`, as a whole
    word;
  - it cannot express "T and everything below T";
  - it cannot rewrite a wildcard;
  - it cannot express loops;
  - it costs every entry one compare per alternative, not only the entries
    below a target;
  - it breaks negation when the alternatives become separate terms.

## State at the handoff

- The path-search session wrote A's link sources and A's matching. The link
  sources are worth keeping: `gozler-winfs` (file ID, ioctl), `links.rs`,
  `index/volume.rs`, `tail.rs`, `server.rs` and the proto types. Then apply
  the tag filter and the journal-reason rule above.
- Replace `query/through.rs`: about 200 lines that step the states and compute
  the fixed point. In `engine.rs`, remove `passes_through` and the numbering
  of path tests across clauses. Add about 30 lines to `MatchedParts` for the
  bold.
- The session's own handoff is `paths-through-links-handoff.md`, and its
  earlier design is `paths-through-links-design.md`, both in this folder.
  Where they disagree with this file, this file decides.
- The first fix, the client rewrite for a path with a drive, built green
  except for one clippy finding in the painting work, which has since been
  fixed. The release `gozler.exe` found 959 matches for
  `C:\SynoSoftware\TinyTorrent2\artifacts\evidence\`, where the installed one
  found 0. This design replaces that fix.

## Other uncommitted work in the same tree

The window-painting work is also uncommitted, in `crates/gozler-ui` and
`docs/DESIGN.md` ("Painting"):
- the presenter covers every window of the UI thread: the Viewer, which now
  draws through a buffer, and the dialogs;
- lists and trees hold their drawing until the update ends (`hold_after`), so
  app code no longer calls `draw_once_after`;
- every `DEFERRED` post goes through `Hwnd::post_deferred`;
- F3 views a file from Search too (`Model::file_to_view`).

It is reviewed, with no open findings. `target\release\gozler-ui.exe`, built at
22:05, holds it for the owner to try: the Viewer (scroll, resize, Select all),
dialogs (Select files, Transfer, drop-down lists), the Folder column of a
search panel, the indexed-folders tree in Settings, and F3 in Search. After the
owner's test, run one full `build.ps1`, then commit when the owner asks.
`docs/DESIGN.md` holds changes from both pieces of work, so stage it by hunk
to keep one commit per feature.

## Build rules for whoever continues

Before any build, list the running `build.ps1` and `cargo` processes. All
sessions share this tree. Do not compile what another session's running or
finished build already covers. Build one release exe for the owner to try, and
run the full `build.ps1` once, only when the owner asks for the commit.
