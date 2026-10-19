---
name: design-ecs
description: Design or change Bevy/ECS components, entities, relationships and systems in this repository.
---

# Design ECS data and transitions

Apply the repository's engineering principles to the ECS model. Do not use ECS
structure to distribute behavior that would be easier to understand together.

## Keep components primarily data

Components describe state and relationships. Accessors, constructors and setters
are appropriate when they preserve local data invariants. Avoid components that
act as miniature behavioral objects communicating extensively with each other.
A reader should not have to reconstruct component-to-component reactions to
understand a transition.

## Keep relationships explicit

Prefer flat entity composition. When hierarchy is part of the domain, express
the necessary relationships as entity references or the existing ECS relationship
mechanism. Do not create elaborate entity nesting merely to organize code.

Identify authoritative state and its derived relationships. Make it clear which
owner maintains each cross-entity invariant and when mutations become visible.
Do not give several systems independent responsibility for the same rule.

## Keep major transitions inspectable

Put consequential ECS behavior in the relevant system and its controlled
callsites. Keep ordering and side effects visible instead of relying on hidden
component reactions or chains of callbacks.

A large coherent system can be preferable to many smaller interacting systems.
Split when the data and transitions become easier to understand independently,
not merely to make each system shorter. Leave necessary local complexity where
it can be inspected rather than hiding it between systems.

Use systems for the scheduled transitions that need them. Preserve the actual
composition boundaries of the affected code; do not introduce scheduling or
system registration around ordinary function calls merely for uniformity. Read
the relevant crate README for current APIs and lifecycle guarantees.
