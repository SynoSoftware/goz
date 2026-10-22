---
name: code-review
description: Review a plan before coding, or review code changes since a Git reference (a branch, PR, commit, or work in progress) against requirements and repository conventions. Also fixes what it finds and reviews again until clean (fix), runs a stronger review with specialists or a second model (deep), and re-reviews only what changed since an earlier review (delta).
---

# Code Review

Find real defects, unclear ownership, and avoidable complexity, and report each one with evidence. The review is read-only unless the user passes `fix`.

## Commands

When the argument is `help` or `?`, print this help as it stands and stop:

```text
/code-review [what] [deep] [fix]

What to review (default: the code you have not committed)
  (nothing)        the code you changed and have not committed
  <git ref>        the code changed since a branch, commit or PR
  plan <plan>      a plan, before anyone writes code: its file, or paste it
  <pass>           one code pass only: spec, paths, flow, architecture,
                   conventions or elegance

How (add any of these)
  deep             a stronger review. A plan gets a second, stronger model
                   that challenges the first review. Code gets six
                   specialists, one for each pass, at the same time.
  fix              fix the findings and review again, round after round,
                   until nothing material remains. Each later round reads
                   only what the fixes changed.
  delta <record>   review only what changed since an earlier review's
                   record, such as after someone else fixed its findings.

Examples
  /code-review                        review my uncommitted code
  /code-review fix                    review it, and fix it until clean
  /code-review main deep fix          strongest review since main, fixed
  /code-review plan docs/plans/x.md deep fix
                                      strongest plan review, revised
                                      until sound

From a request to reviewed code in one step: /feature <what you want>
```

The older words still work: `plan2` is `plan deep`, `code2` is `deep`, `code` is the default review, and `polish` is `fix`.

`fix` on a plan revises only the plan; implementation code stays untouched. Honor the user's limits on tests, builds, execution, and scope.

`deep` delegates to specialists: read [specialists.md](specialists.md) before launching any.

## Preparation, once per review

1. **Plan reviews:** obtain the plan and its intended outcome. Check its premises against the current repository and the workflows it touches. Label a missing detail as an assumption.
2. **Code reviews:** validate the Git reference, or use the merge-base with the default branch. For work in progress, include staged, unstaged, and relevant untracked changes, because `git diff base...HEAD` sees only commits. Leave Git state and unrelated changes as they are.
3. Find the requirements and constraints in the user's input, the issue tracker, commits, or project docs. When none exist, proceed and state what cannot be verified.
4. Read the applicable `AGENTS.md` files, documented standards, nearby implementations, callers, contracts, and established owners. For UI work, find the app's canonical buttons, dialogs, menus, controls, spacing, styles, icons, and tokens.
5. Split a large change or plan into coherent features, and keep shared state and cross-feature interactions visible. A single reviewer finishes one feature before starting the next.

Preparation is done when you can name the diff or plan, its requirements, and the owner of each responsibility it touches. A `delta` review takes its scope and its preparation from the review record instead, as [Review record and `delta`](#review-record-and-delta) says.

## Plan soundness (`plan`)

Challenge the approach before anyone implements it. A plan earns approval only when the repository evidence supports each premise.

1. **Goal and feasibility:** the plan solves the actual requirement within the real platform, dependency, repository, and contract constraints. Name each wrong assumption and missing prerequisite.
2. **Behavior and state:** walk the entry points and the success, failure, retry, and cancellation paths, with their invariants and transitions. Challenge stacked booleans, unsuitable algorithms, implicit ordering, impossible states, and competing validation rules.
3. **Authority and fit:** find the current owner of each responsibility. Challenge duplicate sources of truth, parallel implementations, unneeded abstractions, unrelated redesign, and UI controls that bypass established patterns.
4. **Simplest viable alternative:** for each material weakness, propose the smallest clearer approach and its tradeoff. Propose an enum, state machine, or polymorphism only where the rules need one.
5. **Verifiability:** the plan gives implementers clear boundaries and a way to demonstrate the intended behavior. Flag material risks it leaves open; leave out speculative infrastructure and surplus tests.

Return **Sound**, **Needs revision**, or **Unsound**, with a short rationale, the blocking issues and their repository evidence, the assumptions that remain unverified, and the minimal corrections. With `fix`, return a concise corrected plan.

## Six code-review passes

The default code review runs these passes in order in one agent. `deep` gives each pass to its own specialist, and all six run at once. Each pass is done when its own completion line holds.

### 1. Specification (`spec`)

Map each relevant requirement to the behavior that implements it, or record the gap. Find missing, partial, incorrect, and unrequested behavior. Cite the requirement and the implementation. Separate an ambiguous requirement from a confirmed mismatch.

Done when every requirement has an implementation or a recorded gap.

### 2. Execution paths (`paths`)

Trace each affected entry point through validation, decisions, side effects, persistence, and cleanup. Where they apply, follow the normal, empty, error, cancel, retry, reentrant, concurrent, and shutdown paths. Look for bypassed guards, conflicting eligibility rules, unavailable recovery actions, and regressions. Report a bug only with a concrete path that fails.

Done when every affected entry point is traced.

### 3. Control flow (`flow`)

Question the algorithm and the state representation, not only whether each `if` works. Find stored facts that other state already implies, stacked flags, impossible combinations, repeated decisions, and scattered transitions. Booleans hold independent facts, enums exclusive modes, explicit transitions sequencing that matters, and polymorphism genuinely distinct stable behavior. Recommend the simplest representation that makes legal behavior obvious; a framework or class hierarchy earns its place by the rules it enforces, not by the conditionals it removes.

Done when every changed piece of state has a representation that fits its rules, or a finding.

### 4. Architecture (`architecture`)

Each responsibility has one owner and one authoritative implementation. Check that the change uses the app's existing contracts and owners. Find duplicated business decisions, parallel implementations, misplaced responsibilities, redundant wrappers, and unneeded abstractions. Recommend a refactoring when it materially improves correctness or clarity, inside the scope of the change.

Done when every responsibility the change touches has a named owner.

### 5. Naming and conventions (`conventions`)

Apply the `AGENTS.md` naming rules and repository standards. Compare UI work with the app's real button hierarchy, dialogs, controls, menus, interactions, navigation, spacing, icons, and tokens. Reuse the established solution, and claim compliance only after inspecting real examples.

Done when every new name and UI element has been compared with an existing example.

### 6. Elegance (`elegance`)

Challenge every state value, abstraction, helper, function split, and layer of indirection: what can disappear while behavior stays the same? Prefer intention-revealing names, straightforward algorithms, and code that reads naturally, at a size between giant methods and tiny functions that cost navigation. Recommend a change because it makes the code easier to read or change, never because a smell's name fits.

Done when every added concept in the diff is either needed or reported.

## Review record and `delta`

Every review ends its report with a review record, so that a later review can skip what this one covered and what has not changed since:

- **Reviewed:** for code, each file in the diff, untracked files included, with its `git hash-object` hash. For a plan, a copy of the plan saved outside the repository, such as in the session's scratch or temp directory, because a plan is usually one file, and one changed line would change its hash and send the whole plan back for review.
- **Passes:** the passes or plan checks that reached their completion line.
- **Preparation:** the requirements, the rules applied, and the owner of each responsibility.
- **Settled findings:** each finding with its outcome: fixed, rejected with the reason, or waiting for the user's decision.

A `delta` review, and every round of a fix loop after the first, reviews only what changed since a record:

1. Find the changes. For code, they are the files whose hash differs from the record or that the record lacks. For a plan, they are the sections that differ from the saved copy (`git diff --no-index`). Add what the changes affect: for code, the callers and callees, traced as far as the `paths` pass needs; for a plan, the steps that depend on a changed decision.
2. Reuse the record's preparation. Look up only the owners and rules that the changes newly touch, and any requirement added since the record.
3. Run the requested passes or plan checks on that scope. Everything outside it already passed, so it needs no second read.
4. Keep settled findings settled. Reopen one only with new evidence: the code or plan text it cites has changed, or a failure path its reason does not answer.
5. End with a new record that merges the old record with this review.

The reviewers of a delta did not write the fixes, because a fixer approves its own design choices. Where the host can continue an agent, send the delta to the previous round's reviewers, because they already hold the preparation; otherwise, put the record in a new reviewer's brief. [specialists.md](specialists.md) says how `deep` runs a delta.

Run the full review instead when no record exists, when the record lacks a pass, or when the changes reach shared state or a contract that unchanged owners depend on, because callers traced one at a time can miss an invariant that spans several owners.

## Findings, fixes, and completion

- **Review only:** list findings by severity, each with its pass, file and line, the concrete effect or failure path, the evidence, and a proportional correction. Separate confirmed defects from risks. Report only what affects behavior, ownership, or the cost of changing the code. Give a one-line status for every pass, including a pass that found nothing. Plan reviews return the soundness verdict instead.
- **Review and fix (`fix`):** work in rounds. Round one is the full review the user asked for. In each round, fix each material finding, one coherent problem at a time, preserving unrelated changes; in a plan review, revise the plan. Verify each fix by reading its affected paths before the next fix. Build and run tests as the repository's rules allow; where they limit builds, do it once, after the last round. Reuse the established contracts and UI standards, and stay inside the reviewed change. The next round is a `delta` review of those fixes against the record from before them. Stop when a round reports no material findings.

  Report a finding that needs the user's decision, such as a major refactoring or a product choice, instead of fixing it; it does not keep the loop going. When a finding returns after its fix, stop retrying that finding and report it as unresolved, including the attempted correction. Continue addressing other findings.

  If the host cannot delegate, report the fixes as unreviewed.
- **Final verification:** inspect the final diff, retrace the changed behavior, and run focused checks when the user permits them and they can prove something. Report the number of rounds, what each round fixed, the independent review's result, and what remains unverified.

The review is done when the requirements are met, the affected workflows are coherent, control flow is appropriately modeled, ownership and conventions are respected, and no material avoidable complexity remains.
