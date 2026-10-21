# Stage 7b: native controls for this branch's hand-drawn ones

Owner request 7 in `.work/file-manager-plan.md`; scope settled there.
Source: `.work/custom-controls-report.md` (read its P1-P9 list of what
114d80c corrected on main, and the key bar and panel rows entries).

1. **The key bar.** Stop drawing the F3-F8 push buttons
   (`Button::draw_key`, its `NM_CUSTOMDRAW` handling and anything only it
   uses); Windows draws them as the plain push buttons they already are,
   with the caption "F3 View". Follow DESIGN.md's push-button rule (each
   push button has an icon) the way 114d80c's icon push buttons do, if
   the key bar's buttons can take one without widening the row past the
   minimum window width; say which you chose and why. Update the spec's
   key bar text and DESIGN.md's key bar text only in the stage 8 pass
   (tell the lead what changed); retake the screenshots that show the key
   bar.
2. **The panel rows (decide by one run).** First run the check the report
   describes once: can the Explorer-themed owner-data list view draw its
   own selection and focus while our code sets only the font and colours
   at item prepaint (`CDRF_NEWFONT`), with the taken-entry look and the
   folder-size column unchanged? If yes, remove the row painting the same
   way 114d80c removed it for its gray rows. If the list still paints a
   selection fill we cannot keep, keep the drawing and report what the run
   showed. Either way the look must stay "look A" of the spec.
3. Do not touch the status bar's right group or the 8 controls 114d80c
   converted on main: they stay for main.

Proof: the run's result for item 2, the screenshots of the key bar and
the panel rows in `.work/stage7b/`, `-Test` filters, one full check.ps1.
