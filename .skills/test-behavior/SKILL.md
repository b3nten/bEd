---
name: test-behavior
description: Add, change or assess tests and regression coverage for meaningful behavior and invariants in this repository.
---

# Test meaningful behavior

For a proposed test, identify the useful behavior that would break if it failed:
a user-visible feature, an invariant, a state transition, an intentional failure
or a previously fixed regression. If the answer is only that implementation
shape changed, reconsider the test.

## Choose a useful boundary

Exercise the behavior through a meaningful boundary when practical. Assert
results and effects callers depend on, including failure semantics and state
that must remain intact after a rejected operation.

Avoid requiring a particular private helper, exact mocked call sequence,
internal representation or unobservable implementation order. Assert ordering
when it is part of the behavior or correctness guarantee, not merely how the
current implementation happens to work.

Keep tests compatible with simplifying the implementation. Tests should protect
the contract rather than prescribe an architecture around their mocks.

## Spend maintenance cost where it buys confidence

Tests are maintained code that encode assumptions and constrain later changes.
Add coverage where behavioral protection justifies that cost. A few useful
behavioral tests can be stronger than many brittle tests of private structure;
maximizing test count is not the objective.

Use the relevant verification commands linked from [CONTEXT.md](../../CONTEXT.md)
and the affected crate's README. Start with coverage appropriate to the change;
broaden checks when its reach or an unresolved failure warrants it. Distinguish
what actually passed from commands that were only documented or checks that
could not run.
