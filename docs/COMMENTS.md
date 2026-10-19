# Code comments

These rules apply to every crate. Read them before you add, change or review a
comment or a doc comment.

**Comment only to preserve information a future maintainer needs that the code
does not make clear.** Before adding one, identify what useful knowledge would
be lost without it.

Review each added comment in the final diff. Remove it if a reader can recover
its point from the nearby code, names, types, help text, or another comment. A
true explanation still costs attention when it preserves no otherwise missing
knowledge; this check prevents a file or function from accumulating narration
as it grows.

- **Explain the non-obvious.** State the purpose, constraint, invariant,
  ownership boundary, or consequence that matters. A brief explanation of dense
  logic is useful; narration of straightforward code is not. Describe verified
  behavior, never an invented rationale.
- **Be concise and complete.** Prefer one sentence. Add more only for distinct
  necessary information. Keep every live reason and the conditions that make it
  true; there is no line-count quota. Wrap new or edited comments naturally near
  80 columns, without jagged fragments.
- **Document usage contracts.** A `///` doc comment explains the purpose and
  the caller obligations the signature cannot express: units, side effects,
  failure conditions, ownership or lifetime. Put the failures a caller handles
  under `# Errors`, a panic under `# Panics`, and an `unsafe fn`'s obligations
  under `# Safety`. Each `unsafe` block carries a `// SAFETY:` comment that
  says why its operation is sound; clippy denies a block without one. Public
  visibility alone does not justify a comment that merely repeats the name.
- **Give explanations one home.** Place them beside the code that owns the
  decision; a crate's design goes in the `//!` comment that opens its `lib.rs`
  or `main.rs`. Reference shared rules instead of copying them. For
  workarounds, include the relevant issue and removal condition when known.
- **Keep comments true.** In a doc comment, name code with an intra-doc link
  (``[`Frn`]``), so the CI doc build fails when the name changes. Never cite a
  file and line number, which drift, or a drive or machine only the author
  knows, which tells the next reader nothing. Put no count in a comment unless
  a test asserts it.
- **Describe the software as it stands.** Keep task history, reviewer
  instructions and agent narration in the commit message. Retain historical or
  version details only when they establish a current constraint, such as
  what an older peer on the pipe can read. Add no decorative headings or
  commented-out code.
- **Preserve existing knowledge.** Update, move or remove comments your change
  makes inaccurate or misplaced. Otherwise leave them alone unless comment
  cleanup was requested. During cleanup, compare against the original and
  restore any lost live reason; if its relevance is uncertain, retain it and
  flag the uncertainty. Never perform a bulk rewrite merely to enforce length or
  style.
