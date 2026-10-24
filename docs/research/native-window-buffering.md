# Native window buffering evaluation

**Decision: do not enable `WS_EX_COMPOSITED` on the main window.** The
2026-10-24 release comparison found substantially higher UI-thread CPU cost,
Two panels resize handlers exceeding 200 ms, and a remaining captured toolbar
defect. No production buffering change was retained.

## Controlled comparison

The owner authorized the experiment and keeping it if effective. Both cases
used the same current release test binary, including the retained layout
changes in `8a7e64c` and the existing foreign status changes. Each case created
its own disposable window and fixture. The only experimental difference was
the root HWND's `WS_EX_COMPOSITED` bit. Both applied `SWP_FRAMECHANGED` through
the same checked placement call; native style readback verified the setting.
Existing ListView buffering remained enabled. No DWM animation override,
installed application, service or global setting changed.

Each case performed eight Search resizes, eight splitter motions, typing,
status and mode changes, eight Two panels resizes, eight scrolling operations
over 259 native rows, pane opening and a real 8 MiB copy. Both copies and their
completion assertions passed. The disposable check ran through
`build.ps1 -Timing timing_window_paint_cause`: 14.55 s without buffering and
20.00 s with it. Those test durations include deliberate recording intervals;
they are not handler timings. The first command compiled the optimized test
binaries; the second reused them.

## CPU and handler cost

The fixture used the existing `gozler_winfs::thread_cycles` counter and measured
each handler separately. It also counted UI-thread cycles through the same
three 33 ms message-pump intervals afterward, including deferred paint work.
This interval is not a guarantee that DWM presentation has completed. Values
below are medians of eight operations per row, in millions of CPU cycles.

| Action | Buffering off | Buffering on | Ratio |
| --- | ---: | ---: | ---: |
| Search resize | 93.33 | 241.98 | 2.59× |
| Two panels resize | 93.73 | 762.08 | 8.13× |
| Splitter movement | 45.09 | 190.00 | 4.21× |
| List scrolling | 49.79 | 389.65 | 7.83× |

Median synchronous Two panels resize time rose from **18.95 ms to 211.25 ms**;
handler-only cycles rose from 30.31 million to 396.00 million. Search resize
time rose from 23.58 ms to 38.42 ms. Some other handlers returned sooner while
their later painting cost increased, so handler timing alone would mislead.
Across all 36 measured action intervals, UI-thread cycles totaled 2.86 billion
without buffering and 14.20 billion with it, a 4.96× difference.

Process-cycle measurements were also saved and show the same large cost for
resize and scrolling. They include the test process's background threads;
neither counter includes FFmpeg, DWM or GPU cost. This was one matched pair,
not a general benchmark or an estimate of every user's workload. The repeated
resize/scroll costs and >200 ms handlers are sufficient to reject this option
for a window whose responsiveness takes priority.

## Visual result and evidence limits

The recordings contain 123 baseline and 499 buffered HWND-WGC frames. Frame
counts are not a frame-rate or quality comparison; capture can emit frames
only when content changes. There was no independent desktop recording.

Original buffered frame 7 still shows an interior gap splitting Match path
into Match / ath and a detached search underline. Baseline frame 2 shows the
same failure class. The buffered case therefore fails the existing acquired-
frame criterion. An independent bounded inspection also found coherent panel,
scrolling and Completed samples; it did not quantify a visual improvement.
Inspection stopped at the verified counterexample rather than certifying all
499 frames. No physical-scanout or atomic-publication claim is made.

Saved evidence under `V:/temp/`:

- `gozler-window-composition-{baseline,buffered}.log` and corresponding MKVs;
- `gozler-window-composition-comparison.json` and
  `gozler-window-composition-phase-comparison.json`, including individual samples;
- `gozler-window-composition-{baseline,buffered}-{frames,sheets}/`;
- `gozler-window-composition-buffered-media-audit.md`;
- `gozler-window-composited-check/`, the disposable source and runner.

Both temporary module hooks were removed. All 310 source/build-input hashes
again match the Full53 manifest. No new permanent test or full build was needed
to return to that verified source; only this report and the plan status changed.
The Stage 4/8 proof remains open. This rejects the tested whole-window policy;
it does not prove every native alternative impossible.
