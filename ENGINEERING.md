# Engineering guidance

**Minimize conceptual complexity, not code size.**

The detailed guidance is organized into focused repository skills. Read the
ones relevant to the task; [AGENTS.md](AGENTS.md) contains the shared principles,
priority order and context-maintenance instructions.

- [Model data and invariants](.skills/model-data/SKILL.md): representations,
  relationships, typing, boundary validation and failure semantics.
- [Control state and execution](.skills/control-state/SKILL.md): ownership,
  transitions, mutation, ordering, function locality and side effects.
- [Design abstractions](.skills/design-abstractions/SKILL.md): semantic models,
  APIs, duplication, dependencies and replaceable implementations.
- [Test behavior](.skills/test-behavior/SKILL.md): meaningful boundaries,
  regression protection and the maintenance cost of tests.
- [Review changes](.skills/review-changes/SKILL.md): architecture review,
  refactoring and recognizing distributed complexity.
- [Design ECS](.skills/design-ecs/SKILL.md): data components, entity
  relationships and controlled system behavior.

[CONTEXT.md](CONTEXT.md) identifies the current implementation, decision records
and verification commands.
