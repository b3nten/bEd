---
name: design-abstractions
description: Evaluate or change APIs, shared abstractions, dependencies and extension mechanisms in this repository.
---

# Design abstractions around real concepts

Before adding a helper layer, wrapper, interface, service, factory, dependency or
framework, identify the concrete complexity it removes from the whole system.
Shorter callsites and more organized-looking code are insufficient reasons.

## Generalize semantics

Ask what the operation fundamentally does and whether the proposed users share
that meaning. A useful abstraction expresses a precise domain concept, removes
special cases, centralizes important knowledge or constrains valid behavior.
Moving code behind an interface or into another module does not by itself
provide that abstraction.

Account for every new concept, lifecycle, interaction, dependency, configuration
mechanism and execution path. An abstraction that adds flags, caller-specific
branches, optional callbacks or modes may cost more than independent concrete
implementations.

Keep independent concerns independent. Do not impose a common framework merely
because implementations currently have similar syntax. Independence requires
clear responsibilities and interactions, not a separate class or interface for
every concept.

## Distinguish knowledge from syntax

If two copies encode a rule that must change together, give that knowledge one
owner where practical. Validation rules, state transitions, serialization
assumptions and consequential side effects are particularly important.

If similar code represents concepts that can change independently for different
reasons, small duplication can be cheaper than a new dependency. Do not trade
straightforward repetition for an abstraction that must repeatedly ask which
caller or mode it serves.

## Constrain APIs

Expose what current callers need, including for internal APIs. Restrict
consequential mutators; higher-level intentions can preserve invariants better
than public low-level mutation primitives.

Useful APIs reduce the states and interactions callers must consider. Do not
expose capabilities merely because they might someday be convenient. Broad
interfaces and unrestricted callability expand the program's behavioral surface.

## Prefer replaceable implementations

Build for current requirements. Keep dependencies and implementation assumptions
contained so an implementation can be deleted or replaced without a speculative
plugin system or compatibility layer.

Add extension points when real extensions require them. Avoid interfaces around
one implementation solely for a hypothetical second one, configuration for
constants that do not vary, and hooks with no users. Replaceability does not
require generalized machinery, but it does require avoiding implementation
details spread throughout unrelated code.

A dependency adds concepts, behavior, assumptions, update costs and interactions.
Use one when it removes more complexity than it introduces. Convenience for a
small operation does not by itself justify an opaque library or framework.
