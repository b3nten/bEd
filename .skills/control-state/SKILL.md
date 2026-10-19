---
name: control-state
description: Implement or change state transitions, consequential mutation, side effects and execution ordering in this repository.
---

# Control state and execution

Make the cause, prior state, transition, resulting state and significant effects
of an operation inspectable together. Use the execution mechanism that makes
those facts clear; no particular reducer, state machine or command pattern is
required.

## Give important state one conceptual owner

Identify who owns the state, who may read or write it, which invariants involve
it and where its transitions occur. Prefer one conceptual writer for important
or tightly constrained state, even when several implementation functions help
that owner perform a transition.

Other subsystems can supply facts, intentions, observations or requests for the
owner to resolve. Do not let unrelated callers each implement part of a
cross-system mutation. Use the existing direct execution path when it already
provides that control; a new request framework is not an end in itself.

Keep invariant enforcement and consequential behavior authoritative in as few
places as practical. This centralizes responsibility for a particular behavior,
not all application code.

## Keep stateful execution visible

Keep one-use stateful operations local when extracting them would hide mutation,
ordering, dependencies, assumptions, errors or effects. Scopes, whitespace,
comments and descriptive names can make coherent procedural stages readable.
A large function or system can be easier to understand than many tiny helpers.

Inline a one-use helper when doing so reveals the operation. Extract pure
computation more readily: take explicit inputs, return values and leave the
important mutation at its controlled execution point.

Avoid unnecessary branching and deeply nested conditions. Prefer one consistent
path with a clearly gated conditional operation when the operation is otherwise
shared. Do not use flags to force unrelated operations through the same path.

Do not fragment execution merely to avoid small amounts of harmless work.
Optimize for actual performance requirements while preserving ownership and
execution visibility wherever possible.

## Make effects deliberate

Treat callability as permission to cause behavior. Keep APIs that mutate important
state, perform I/O, change ownership, create or destroy resources, or affect
persistence available only to their intended callers.

Do not hide consequential work inside a harmless-looking getter, constructor,
registration call or helper. Representation encapsulation is useful, but control
over who can cause important behavior takes priority over rigid field privacy.

Events, hooks, observers and callbacks are permitted when their semantics fit.
For an important reaction chain, make its producer, consumers, ordering, effects
and invariant owner identifiable. If understanding a transition requires
reconstructing a distributed reaction graph, prefer controlled orchestration.
Local complexity that can be inspected is often cheaper than complexity hidden
between interacting subsystems.
