---
name: model-data
description: Design or change data structures, relationships, types, validation and error semantics in this repository.
---

# Model data and invariants

Start with the data and its meaning before choosing objects, modules, services
or frameworks. Let the representation follow the actual relationships and
transformations.

## Establish the model

For the part of the program being changed, identify:

- What data exists, what it means and which relationships are fundamental.
- What changes, what is stable and who needs to read or write it.
- Which transformations occur and what must remain true before and afterward.

Look for the underlying concept behind concrete requirements. A better model
can eliminate apparent special cases. Generalize when the distinction actually
disappears, rather than moving exceptions into configurable machinery. Keep
independent concepts independent even when their current representations look
similar.

## Choose representations deliberately

Before adding validation or recovery, determine whether the invalid state needs
to be representable. Prefer a better model, constructor, ownership rule or
constrained API when it removes the error entirely.

Use stronger types when the data is stable, valid states are understood and
mechanical guarantees simplify reasoning. Allow evolving schemas and genuinely
variable data to remain flexible when extra type machinery would cost more than
the guarantees it provides. Neither type sophistication nor permissiveness is a
goal by itself.

Separate local invariants from invariants involving several pieces of state:

- Establish local invariants through the data structure's constructors or a
  small set of valid mutations.
- Give cross-system invariants an owner with enough context to enforce them.
  Do not make every participating subsystem independently maintain the rule.

## Validate external uncertainty once

Validate user input, files, database records, serialized data, network messages,
remote APIs and external-process results at a clear boundary. Convert accepted
input into a trusted internal representation with meaningful errors on failure.

Internal code should rely on those established guarantees. Repeat validation
only when there is a concrete reason an invariant can legitimately become
invalid. If internal code can freely break it, fix the mutation ownership or
transition design instead of scattering defensive checks.

## Give failures explicit semantics

Treat failure as an intentional result of execution, with understandable
propagation and effects on state. Do not obscure failure behavior merely to
reduce error-handling code.

When an internal invariant is violated, expose the broken assumption clearly.
Do not guess missing values, substitute arbitrary defaults or silently repair
relationships. Recovery belongs where the application deliberately defines what
recovery means; permissive internal APIs must not invent it accidentally.
