---
name: code-review
description: Review a plan before coding, or review code changes since a Git reference (a branch, PR, commit, or work in progress) against requirements and repository conventions. Also fixes what it finds when asked (polish), and can run its passes as parallel specialist reviews.
---

# Code Review

Find real defects, unclear ownership, and avoidable complexity, and report each one with evidence. The review is read-only unless the user passes `fix` or `polish`.

## Commands

Pass one mode to `/code-review` (or the host's equivalent invocation):

| Mode | Review |
| --- | --- |
| `plan` | One reviewer challenges a plan before coding. |
| `plan2` | A reviewer (Opus / Sol) reviews a plan, then a challenger (Fable / Astra) challenges its findings. |
| `spec`, `paths`, `flow`, `architecture`, `conventions`, `elegance` | One code-review pass, by its name. |
| `code` (default) | All six passes, in order, by one reviewer. |
| `code2` | All six passes in parallel, one specialist per pass, with the model chosen by difficulty. |

For code modes, an optional Git reference selects the diff base, and `fix` or `polish` authorizes corrections. For plan modes, the argument is the plan or its location, and `fix` revises the plan; implementation code stays untouched. Honor the user's limits on tests, builds, execution, and scope.

`code2` and `plan2` delegate to specialists: read [specialists.md](specialists.md) before launching any.

## Preparation, once per review

1. **Plan modes:** obtain the plan and its intended outcome. Check its premises against the current repository and the workflows it touches. Label a missing detail as an assumption.
2. **Code modes:** validate the Git reference, or use the merge-base with the default branch. For work in progress, include staged, unstaged, and relevant untracked changes, because `git diff base...HEAD` sees only commits. Leave Git state and unrelated changes as they are.
3. Find the requirements and constraints in the user's input, the issue tracker, commits, or project docs. When none exist, proceed and state what cannot be verified.
4. Read the applicable `AGENTS.md` files, documented standards, nearby implementations, callers, contracts, and established owners. For UI work, find the app's canonical buttons, dialogs, menus, controls, spacing, styles, icons, and tokens.
5. Split a large change or plan into coherent features, and keep shared state and cross-feature interactions visible. A single reviewer finishes one feature before starting the next.

Preparation is done when you can name the diff or plan, its requirements, and the owner of each responsibility it touches.

## Plan soundness (`plan`, `plan2`)

Challenge the approach before anyone implements it. A plan earns approval only when the repository evidence supports each premise.

1. **Goal and feasibility:** the plan solves the actual requirement within the real platform, dependency, repository, and contract constraints. Name each wrong assumption and missing prerequisite.
2. **Behavior and state:** walk the entry points and the success, failure, retry, and cancellation paths, with their invariants and transitions. Challenge stacked booleans, unsuitable algorithms, implicit ordering, impossible states, and competing validation rules.
3. **Authority and fit:** find the current owner of each responsibility. Challenge duplicate sources of truth, parallel implementations, unneeded abstractions, unrelated redesign, and UI controls that bypass established patterns.
4. **Simplest viable alternative:** for each material weakness, propose the smallest clearer approach and its tradeoff. Propose an enum, state machine, or polymorphism only where the rules need one.
5. **Verifiability:** the plan gives implementers clear boundaries and a way to demonstrate the intended behavior. Flag material risks it leaves open; leave out speculative infrastructure and surplus tests.

Return **Sound**, **Needs revision**, or **Unsound**, with a short rationale, the blocking issues and their repository evidence, the assumptions that remain unverified, and the minimal corrections. With `fix`, return a concise corrected plan.

## Six code-review passes

`code` runs these passes in order in one agent. `code2` gives each pass to its own specialist, and all six run at once. Each pass is done when its own completion line holds.

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

## Findings, fixes, and completion

- **Review only:** list findings by severity, each with its pass, file and line, the concrete effect or failure path, the evidence, and a proportional correction. Separate confirmed defects from risks. Report only what affects behavior, ownership, or the cost of changing the code. Give a one-line status for every pass, including a pass that found nothing. Plan reviews return the soundness verdict instead.
- **Review and polish (`fix`, `polish`):** work in rounds. In each round, fix each material finding, one coherent problem at a time, preserving unrelated changes; in a plan mode, revise the plan. Verify each fix by reading its affected paths before the next fix. Build and run tests as the repository's rules allow; where they limit builds, do it once, after the last round. Reuse the established contracts and UI standards, and stay inside the reviewed change. Recheck the affected behavior and earlier findings after each round. Review the whole change again only when fixes introduce broader risks. Stop when no material findings remain.

  Report a finding that needs the user's decision, such as a major refactoring or a product choice, instead of fixing it; it does not keep the loop going. When a finding returns after its fix, stop retrying that finding and report it as unresolved, including the attempted correction. Continue addressing other findings.

  An agent that did not write the fixes reviews the fix diff before the change counts as passed, because a fixer approves its own design choices. Its findings start another round, and its next review covers only the new fixes. If the host cannot delegate, report the fixes as unreviewed.
- **Final verification:** inspect the final diff, retrace the changed behavior, and run focused checks when the user permits them and they can prove something. Report the number of rounds, what each round fixed, the independent review's result, and what remains unverified.

The review is done when the requirements are met, the affected workflows are coherent, control flow is appropriately modeled, ownership and conventions are respected, and no material avoidable complexity remains.
