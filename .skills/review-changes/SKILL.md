---
name: review-changes
description: Review code or architecture and plan or assess refactoring for conceptual complexity in this repository.
---

# Review changes for conceptual complexity

Assess the reasoning burden of the whole design. Architectural sophistication,
small functions, fewer lines and stylistic symmetry are not evidence of a simpler
program. Follow the priority order in [AGENTS.md](../../AGENTS.md) when goals
conflict.

## Preserve behavior and useful architecture

When modifying existing code, preserve required behavior and established
ownership unless the task gives a meaningful reason to improve them. Avoid
unrelated abstractions or mechanical cleanup. Preserve a deliberate single-writer
model and localized execution; do not replace direct transitions with reactive
interactions merely because that pattern is available.

When a change exposes a deeper semantic problem, address the underlying concept
when justified by the task. Do not begin by inventing a framework around it.

Judge a refactor by whether it reduces concepts, states, mutation sites,
execution paths, dependencies or invariant enforcement points, or makes data,
ownership, transitions, effects and replacement clearer. Added concepts must
remove greater complexity elsewhere.

## Inspect the relationships that carry behavior

Use the questions relevant to the change:

- Where does important state live, who can mutate it and how many conceptual
  writers are there?
- What are the invariants, and where is each established and maintained?
- Which callsites can trigger consequential effects? Can a reader follow their
  execution order and identify why an effect happened?
- Do callbacks, events, helpers or deep call chains obscure that execution?
- Does the representation match the data and its actual relationships? Can an
  invalid state be removed rather than repeatedly checked?
- Is external uncertainty validated at a clear boundary that internal code can
  trust?
- Do abstractions represent shared meaning, or couple unrelated concepts through
  modes and configuration? Do extension points have actual users?
- Could an API be narrower or an abstraction be deleted? Could an implementation
  be easier to replace instead of more extensible?
- Do tests protect behavior and important failures while allowing internal
  simplification?

Do not criticize a coherent function, reducer or ECS system merely for being
large. Explicit procedural stages, local concrete implementations and small
amounts of harmless syntax duplication can all be appropriate.

## Recognize distributed complexity

Investigate designs where a simple change crosses many independent layers,
every subsystem knows the others, state has many writers, rules are checked
repeatedly, helpers require unrelated parameters, or debugging means tracing
several reaction chains. Difficulty deleting a subsystem and tests that break
on private rearrangements are further signs of unnecessary coupling.

First consider simplifying the design; documentation alone does not remove that
complexity. Prefer the implementation with fewer surprising behaviors, clearer
owners and effects, fewer dependencies and hypothetical capabilities, and an
execution path that will be easier to debug.
