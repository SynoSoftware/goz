# Stage 4, round 4 (from the lead)

The recheck of round 3 confirms M1, L3, L4, W1, W3, the running totals
and the batch-per-side work. Two items remain; do them together with #105,
then the affected `-Test` filters and one full `check.ps1`.

**N1 (medium, from W2).** An entry hidden by a delete or move keeps its
selection mark, and the mark later re-selects it without the person's
action:
1. Select `a.txt` with Insert.
2. In Search, delete it. `filter` hides it, and its mark is kept.
3. The delete fails, so `a.txt` shows again. `place_of` is skipped,
   because the selection is empty.
4. Back in Two panels, the reread carries the mark.
5. Select `b.txt`.
6. The next change runs `place_of`, finds both marks, and selects `a.txt`.

Fix: clear the mark of every entry that `filter` hides (all marks when the
whole folder is taken). Add a core test for the sequence.

**N2 (low) and the fallback pass.** Lead ruling: a name the watch reports
as added, or as the new name of a rename, is the name as stored on disk,
so it matches by exact name only. The case-ignoring fallback stays only for
removed names and the old names of renames. (`stat` already reads
`final_path` for `~` names; that is enough for short names.) This fixes the
WSL case (`makefile` added beside `Makefile` no longer removes it) and
removes the second full HashMap pass that the fallback made for every
newly created file. The rename check in `apply_changes` (the
focus-follows-rename logic) follows the same rule.

**Window-thread budget per change batch** (lead ruling, in the plan): a
simple pass over the entries with no allocation and no name folding per
entry is acceptable (a few ms at a million entries); allocation or folding
per entry is not. So `place_of` scanning shown rows while something is
selected is acceptable; restrict it to the selected rows only if that is
simple.

End your turn with #105's result, N1 and N2, the tests, and the check
result.
