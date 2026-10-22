# Specialist reviews (`deep`)

How the coordinator runs a `deep` review. The passes and the plan checks themselves live in [SKILL.md](SKILL.md).

## Orchestration

One coordinator owns the scope, the findings, and every edit. Specialists only read and report.

1. **Prepare once.** Do the preparation in SKILL.md, then give every specialist the same evidence: the diff or plan, the requirements, the applicable `AGENTS.md` rules, the owners found, and the settled findings of any earlier review record, which stay settled as SKILL.md says.
2. **Launch.** For code, start all six specialists at once, in one message where the host supports it. No specialist waits for another, because each pass reads the same change through its own lens and needs no other pass's result. A plan review runs in series, as its section below describes.
3. **Merge.** When all have reported, merge duplicates (the passes overlap, so one defect often arrives from two passes) and challenge each finding that lacks a concrete failure path or evidence. Send a disputed or high-risk finding to the stronger model its routing row names.

With `fix`, every round after the first is a `delta` review, as SKILL.md says. For code, one Opus / Sol reviewer runs all six passes on the delta, because a fix diff is small and six specialists would each read the shared evidence again for it. Launch the six specialists again only when SKILL.md calls for the full review. For a plan, the reviewer and then the challenger review the delta.

Each specialist's brief holds its pass section from SKILL.md (or, for a plan, its role below), the shared evidence, and the findings format from SKILL.md. Ask for findings only, in plain words. A specialist is done when its pass's completion line holds and it has reported a status, even when it found nothing.

If the host cannot delegate, or a named model is unavailable, run the same passes in the current agent, in order, and say so in the report.

## Plan review (`plan deep`)

Two specialists review the plan in series:

1. The reviewer (Opus / Sol, default tier) reviews the plan with the plan soundness checks in SKILL.md and reports its findings.
2. When it has reported, the challenger (Fable / Astra, top tier) receives the plan, the shared evidence, and those findings. It confirms or refutes each finding with evidence, and questions the architecture, the control-flow model, and the hidden assumptions the first review missed.

The coordinator resolves what remains disputed with evidence and returns one verdict.

## Models

These are routing preferences for the model family the host actually offers. Use only the delegation tools and models the host has.

| Role | Claude family | OpenAI family |
| --- | --- | --- |
| Top: highest judgment, highest cost | Fable | Astra |
| Default: most passes | Opus | Sol |
| Economy: routine review | Sonnet | Terra |
| Mechanical checks only | | Luna6 |

Reserve Fable and Astra for consequential or disputed conclusions. Luna6 is far weaker than Terra: give it preparation, inventory, and narrow checks that a stronger reviewer verifies, and never a whole pass.

## Code review routing (`deep`)

Start each pass at its routine model, and escalate a pass or a single finding when it proves complex or disputed:

| Pass | Routine | Complex or uncertain |
| --- | --- | --- |
| Spec | Sonnet / Terra | Opus / Sol |
| Paths | Opus / Sol | Fable / Astra when high-risk or disputed |
| Flow | Opus / Sol | Fable / Astra for hard state or algorithm decisions |
| Architecture | Opus / Sol | Fable / Astra for consequential decisions |
| Conventions | Sonnet / Terra | Opus / Sol for conflicting patterns |
| Elegance | Opus / Sol | Fable / Astra for hard design tradeoffs |
