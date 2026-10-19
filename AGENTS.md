# Repository guidance

Read [CONTEXT.md](CONTEXT.md) at the start of a task. It identifies the current
implementation, relevant decision records and verification commands. Read the
README for the crate you are changing, then load the applicable engineering
skills below. Preserve existing uncommitted work.

## Engineering principles

**Minimize conceptual complexity, not code size.**

Model the data and relationships first.

Keep execution visible.

Keep state transitions explicit.

Localize consequential behavior.

Give important state a clear owner.

Maintain each invariant in as few places as possible.

Make invalid states difficult to represent.

Validate untrusted data at boundaries, then trust the resulting internal representation.

Prefer pure functions for computation and controlled execution points for mutation.

Do not create abstraction merely to organize code.

Do not create indirection merely to make functions smaller.

Do not create dependencies merely to remove similar-looking syntax.

Do not create flexibility merely because future requirements are unknown.

Make implementations deletable rather than prematurely extensible.

Test meaningful behavior rather than private structure.

When a choice must be made, prefer the design that reduces the number of states, interactions, owners, dependencies, and execution paths that a programmer must keep in their head.

Above all:

**Make it obvious what the program can do, when it can do it, why it does it, and where its important state can change.**

When principles conflict, prioritize correctness and required behavior, clear
ownership, visible execution, fewer states and interactions, accurate data
modeling, localized side effects, fewer dependencies and indirection, and
replaceability, in that order. Reuse and aesthetic preferences come afterward.

## Engineering skills

The guides live in `.skills/`. Read the linked `SKILL.md` files directly when
their subject applies; this routing does not depend on automatic discovery of
that directory. Load only the skills relevant to the task.

| When working on | Read |
| --- | --- |
| Data structures, relationships, types, validation or errors | [Model data](.skills/model-data/SKILL.md) |
| State transitions, mutation, side effects or execution order | [Control state and execution](.skills/control-state/SKILL.md) |
| APIs, abstractions, dependencies, reuse or extension points | [Design abstractions](.skills/design-abstractions/SKILL.md) |
| Tests and regression coverage | [Test behavior](.skills/test-behavior/SKILL.md) |
| Code review, architecture review or refactoring | [Review changes](.skills/review-changes/SKILL.md) |
| Bevy/ECS components, entities, relationships or systems | [Design ECS](.skills/design-ecs/SKILL.md) |

[ENGINEERING.md](ENGINEERING.md) is an index to these guides.

## Maintaining context

- Keep `CONTEXT.md` short and current when direction, unfinished work or useful
  verification commands change. Update existing entries and remove stale ones.
- Record durable decisions and their reasons in the relevant crate README or
  migration document alongside the implementation change. Link to that record
  from `CONTEXT.md` instead of copying it.
- Put surprising code invariants beside their owner. Record failed approaches
  only when the reason and evidence would prevent repeating useful discovery.
- Distinguish current behavior, accepted plans and unverified hypotheses. Record
  the date and evidence for claims that can become stale; never imply a check
  passed unless it ran successfully.
- Save information that would change a future implementation decision or avoid
  substantial rediscovery. Keep transcripts, routine command logs and temporary
  progress out of durable context.
- Use one `CONTEXT.md` as the entry point. Add a focused linked document only
  when a substantial topic needs its own maintained record.
