# Linked path search, 2026-10-29

The reported missing path was below the junction
`C:\SynoSoftware\TinyTorrent2\artifacts`, whose target is
`V:\artifacts\TinyTorrent2`. `release` is an ordinary folder below it.
The project contains that junction and 159 folder symbolic links, mostly in
Boost. The live Gozler index held about 2.33 million entries on C: and V:.

The earlier links feature, `9c1d086`, preserved the component-based Match path
scan but left separator-containing path terms scanning every live entry.
The candidate-walk change uses the name arena to find the last required
literal within a component, visits those entries and their descendants, and
includes a link target when its reaching matcher state has consumed that
literal. The existing matcher verifies the full query. Both total traversal
and candidate evaluation are bounded; overlapping candidates count once.

## CPU measurements

Criterion counted the benchmark thread's CPU cycles, with 20 samples per
case. These are point estimates in millions of cycles, not milliseconds.
Both revisions used the same machine, synthetic index and optimized profile.
The ordinary query fixture has 2,141,354 live entries. The linked-release
fixture adds a target subtree with 17 files and models the junction above.
It explicitly sets `ParseOptions::match_path` and constructs `Links` and
`CompiledQuery::with_links` inside each timed iteration. Every iteration
asserts 17 results. Index construction and ranking are outside the timing.

| Query | Links feature before optimization | Candidate walk | Ratio |
| --- | ---: | ---: | ---: |
| `module_group_007\bin_x64_03` | 506.76 | 81.583 | 6.2x |
| Same query, one unrelated junction | 497.96 | 101.84 | 4.9x |
| `artifacts\release\` | 476.24 | 12.033 | 39.6x |
| `C:\SynoSoftware\TinyTorrent2\artifacts\release\` | 401.95 | 13.932 | 28.9x |
| `C:\SynoSoftware\TinyTorrent2\artifacts\release\*.exe` | 3864.1 | 21.245 | 181.9x |

The 95% intervals for the three optimized release cases were respectively
11.691–12.283, 12.736–15.120 and 19.311–23.264 million cycles.

The pre-feature revision `b275ba2` cost 308.03 million cycles for the first
path term; repeating its existing binary on one CPU core gave 378.10 million.
The links-feature binary repeated on that core gave 462.69 million. Thus the
candidate walk also beats the measured pre-feature path scan. The 308 million
path result and 15 million `-p kernel32` result are different queries, not
the two sides of a regression comparison.

The optimized `-p kernel32` case cost 20.755 million cycles and the name-only
case 22.087 million. Criterion detected no statistically significant change
against the latest links-feature samples. Repeated baseline runs varied
enough that these figures do not establish preservation to a small percentage.

The linked-release fixture does not time the daemon's extraction and
canonicalization of the complete live link table, permission checks, or pipe
transport. It has one synthetic link; it is not a many-link scaling result.

## Live service measurement before optimization

Two warmups and nine measured runs per query, alternating case order, timed
the release CLI against the existing service. Wall time includes process
startup, handshake, daemon table preparation, full matching/counting and pipe
reply. These figures are not interchangeable with core CPU cycles.

| Query shape | Median milliseconds | Matches |
| --- | ---: | ---: |
| Absolute linked release path | 378.85 | 17 |
| Physical V: release path | 365.80 | 17 |
| Linked release path with `*.exe` | 1700.49 | 8 |
| Physical release path with `*.exe` | 1548.57 | 8 |
| `-path` linked release folder | 26.01 | 17 |
| `-path` physical release folder | 26.23 | 17 |
| `-p kernel32` | 26.62 | 279 |

In this first live set, separator-containing terms selected path matching
implicitly; `kernel32` used explicit `-p`. An additional live run sets `-p`
explicitly on every query. That run returned the same 19 entries for the
linked and physical release paths, and the same nine executables for their
wildcards. Its medians were 445.37/453.92 ms for linked/physical paths,
2270.21/1844.93 ms for wildcards and 28.62/28.26 ms for folder scopes. The
directory gained two release files during the session, so live checks compare
linked and physical result counts rather than asserting that the mutable
directory permanently contains 17 entries. Shared-machine wall times and the
live index changed; this second set is not a before/after code comparison.

The optimized code was not installed over the running service. Its speedups
above are core benchmark results, not a measured deployed-service speedup.

## Reproduction and verification

All compilation used `build.ps1`; no dependency was added. The baseline
snapshots came from `git archive`, leaving the shared checkout unchanged.
For the pre-optimization linked-release fixture, copy the same benchmark
source into revision `9c1d086` and use a distinct temporary package version:
sharing Cargo artifacts between archived revisions with the same package
identity initially selected a stale pre-feature library. That setup failure
was corrected before accepting any linked-release baseline result.

```powershell
pwsh ./build.ps1 -Bench 'query_2M/(rare|match-path.kernel32|path-term)'
pwsh ./build.ps1 -Bench linked_release_2M
pwsh ./build.ps1 -Bench '(query_2M/(rare|match-path.kernel32|path-term)|linked_release_2M)'
```

Four `build.ps1 -Bench` runs completed: the ordinary query cases on the
pre-feature and links-feature revisions, the linked-release baseline, and
the combined optimized cases. One earlier linked-release baseline attempt
failed at compilation because of the shared-artifact setup described above;
it supplied no accepted measurement. Existing benchmark binaries were also
repeated on one CPU core without recompilation. No focused `-Test` run was
needed for this optimization.

The added regression compares candidate-scan entry IDs, including multiplicity,
with exhaustive evaluation. It watches lost matches across links, partially
consumed literals, wildcards, and duplicate matches from overlapping roots.
The existing randomized path and link-state checks also remain in place.
Independent source review found and corrected a traversal budget that had
counted only unique entries; it now counts every yielded entry before
deduplication, bounding repeated traversal of nested candidates.

The first full verification passed all 860 tests, with 15 timing checks
ignored, and documentation generation. Clippy reported one collapsible `if`,
which was corrected. The second full attempt passed formatting and Clippy
but was interrupted during test compilation. The third attempt ran in the
sandbox, where existing pipe, ACL, token and working-set tests failed on
restricted OS operations; this is an execution restriction, not evidence of
a query defect. The fourth full attempt, outside the sandbox, finished green:
860 tests passed, 15 timing tests were ignored, and formatting, Clippy with
warnings denied and documentation with warnings denied all passed. No source
edit was needed for the restricted-operation failures. There were four full
attempts in total, including the interrupted attempt; no further compilation
was needed after this result.
Ubuntu and minimum-Rust verification remain for CI; no dependency or purity
allowlist changed.
