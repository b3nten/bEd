# Software Engineering Rules for AI Coding Agents

## Local validation workflow

- During iteration, run the smallest relevant test target and test filter. For Bed,
  use `cargo bed-test --lib FILTER` or `cargo bed-test --test TARGET FILTER`.
  Bed has one fixed application composition, including automation.
- For reusable crates, select the affected package and the feature set needed by
  the changed behavior. Do not sweep feature combinations after ordinary edits.
- Once relevant checks pass, stop testing unless another change, a failure, or an
  unresolved concern warrants more checks. Run `cargo test-full` only when the
  user requests broad validation or a cross-workspace change requires it; CI runs
  the full suite and reusable-crate feature matrix.
- Native window, focus, menu, and GPU smokes belong to explicit desktop validation.
  Do not launch them during routine coding or headless test work. They can focus
  windows and consume the user's typing. Run them locally only when the user
  explicitly requests that smoke session; otherwise use CI.
- Avoid changing profiles, RUSTFLAGS, target directories, or reusable-crate feature sets
  between routine reruns. Cargo caches variants, but switching can require
  separate dependency builds and links. Do not clear caches or disable macOS
  executable verification to speed up tests.

## Purpose of This Document

These rules define how you, the coding agent, should reason about software design, implementation, refactoring, testing, and architecture.

These are not merely formatting preferences or stylistic conventions. They describe a philosophy for controlling software complexity.

The highest-level objective is:

**Make the program easy to reason about.**

The primary enemy is not long code, large functions, duplication, mutable state, classes, weak typing, strong typing, callbacks, events, abstraction, or any particular architectural pattern in isolation.

The primary enemy is **conceptual complexity**: complexity that makes it difficult to know:

* what the program can do;
* when it can do it;
* what state the program can be in;
* where important behavior occurs;
* which code is allowed to change important state;
* which invariants must remain true;
* where those invariants are enforced;
* what caused a particular side effect;
* what other parts of the program a change may affect.

Every architectural and implementation decision should be evaluated against this standard.

Do not confuse code that looks organized with a system that is simple.

Do not confuse small functions with simplicity.

Do not confuse abstraction with simplicity.

Do not confuse reuse with simplicity.

Do not confuse flexibility with simplicity.

Do not confuse brevity with simplicity.

Do not confuse familiarity with simplicity.

Prefer designs that reduce the number of concepts, states, interactions, owners, and possible execution paths that a programmer must understand.

---

# 1. Optimize for Conceptual Simplicity

When choosing between two implementations, prefer the implementation that requires the programmer to understand fewer independent concepts and fewer relationships between those concepts.

Prefer:

* fewer concepts;
* fewer possible states;
* fewer important execution paths;
* fewer places where meaningful behavior can occur;
* fewer places where important state can be mutated;
* fewer places where invariants must be maintained;
* fewer hidden dependencies;
* fewer unexpected side effects;
* fewer arbitrary relationships between systems;
* more explicit execution order;
* stronger and clearer boundaries;
* more obvious ownership;
* more obvious cause-and-effect relationships.

The purpose of architecture is not to maximize the number of architectural constructs. Architecture should reduce the number of things that need to be understood simultaneously.

A solution that uses fewer abstractions may be better than one that appears more architecturally sophisticated.

A solution containing a 200-line coherent function may be simpler than a solution containing twenty small functions connected by hidden state, callback relationships, and control flow.

A large reducer may be simpler than twenty modules capable of mutating the same state.

A single explicit update loop may be simpler than an elegant collection of observers and event handlers.

A straightforward conditional may be simpler than introducing a polymorphic hierarchy.

Physical code size is not conceptual complexity.

Do not refactor merely to make files, classes, methods, or functions smaller.

Refactor when doing so makes the **system easier to reason about**.

---

# 2. Do Not Mistake Convenience for Simplicity

Something may be convenient to write while making the resulting system more complicated.

An abstraction may reduce five lines at one callsite while adding:

* another concept;
* another interface;
* another lifecycle;
* another dependency;
* another configuration mechanism;
* another extension mechanism;
* another path through the program.

Do not judge an abstraction only by whether the immediate caller becomes shorter.

Evaluate its effect on the whole system.

Before adding any of the following:

* a helper layer;
* a wrapper;
* an event system;
* a plugin interface;
* middleware;
* a service abstraction;
* a generalized framework;
* a dependency;
* a configuration mechanism;
* a factory;
* a callback registry;
* an interface around a single implementation;

ask whether the new construct makes the **underlying problem simpler**.

If it merely moves complexity elsewhere, it has not removed complexity.

If it merely hides execution, it may have made the program harder to understand.

If it primarily prepares for hypothetical future requirements, do not add it.

---

# 3. Avoid Complecting Independent Concerns

Do not unnecessarily intertwine things that could remain independent.

When two concepts are independent in the problem domain, try to keep them independent in the implementation.

Avoid designs where changing one concept requires understanding several unrelated concepts.

Avoid abstractions that force unrelated behaviors through the same machinery merely because their code currently looks similar.

Avoid central frameworks that cause unrelated systems to become coupled to one generalized mechanism without a compelling semantic reason.

A good system has pieces that can remain independent while their interactions remain obvious.

Independence does not mean every piece must have its own class, module, interface, or abstraction.

Independence means a concept should not acquire unnecessary obligations or dependencies on unrelated concepts.

---

# 4. Clear Is Better Than Clever

Prefer code whose behavior is immediately understandable.

Do not introduce cleverness merely to reduce line count.

Do not favor a sophisticated technique when a direct technique communicates the behavior more clearly.

Do not optimize for impressiveness.

Do not optimize for novelty.

Do not optimize for abstract elegance at the expense of execution clarity.

When a reader asks, “What happens when this runs?”, the code should make the answer as obvious as practical.

---

# 5. Begin With Data, Relationships, and Transformations

Programming is fundamentally about data, relationships between data, and transformations of that data.

Before designing:

* classes;
* objects;
* interfaces;
* inheritance hierarchies;
* modules;
* services;
* managers;
* controllers;
* frameworks;

first understand:

1. What data exists?
2. What does the data mean?
3. What relationships exist between pieces of data?
4. Which relationships are fundamental?
5. Which data changes?
6. Which data is stable?
7. Who needs to read each piece of data?
8. Who needs to write each piece of data?
9. What transformations occur?
10. What invariants must remain true before and after those transformations?

The structure of the program should emerge from these answers.

Do not design an object model before understanding the data model.

Do not force data into a hierarchy merely because object-oriented constructs are available.

Do not choose an architecture before understanding access patterns and transformations.

The data and its relationships should strongly influence the program structure.

---

# 6. Choose Typing According to the Data

Typing is a tradeoff rather than an unconditional virtue.

Stronger typing can:

* prevent invalid states;
* document assumptions;
* make guarantees mechanically enforceable;
* improve local correctness.

But stronger typing also creates concretions.

Those concretions may make constantly changing or highly variable data harder to work with.

Weaker or more permissive typing can accommodate:

* evolving schemas;
* highly variable data;
* many legitimate permutations;
* data whose shape is intentionally dynamic.

But weaker typing makes some correctness guarantees harder to establish.

Therefore:

**Choose the typing strategy according to the nature of the data.**

Prefer stronger typing when:

* the data is stable;
* the valid states are well understood;
* invalid states should be unrepresentable;
* strong guarantees are valuable.

Prefer weaker or more permissive typing when:

* the data changes frequently;
* the set of valid permutations is large;
* the schema is legitimately dynamic;
* excessive type machinery would create more conceptual burden than correctness value.

Do not maximize type sophistication as an end in itself.

Do not weaken types merely for convenience either.

Use types to simplify reasoning about the actual data.

---

# 7. Search for the Underlying Problem

Requirements are often presented as concrete cases.

Do not assume the concrete request is the fundamental problem.

When appropriate, ask what underlying concept makes the concrete requirement necessary.

A difficult special case can sometimes become trivial when recognized as one instance of a more general semantic model.

The goal is not to generalize indiscriminately.

The goal is to discover a model in which apparent special cases cease to be special cases.

When multiple requirements appear unrelated, investigate whether they are instances of a smaller number of concepts.

A strong generalization removes exceptional behavior.

A weak generalization merely routes exceptions through a configurable abstraction.

Prefer the former.

---

# 8. Generalize Semantics, Not Machinery

When searching for an abstraction, first search for a better semantic model.

Do not begin by asking:

* “Can these classes share a base class?”
* “Can these functions use the same interface?”
* “Can we create a generic handler?”
* “Can this become a plugin?”
* “Can this be parameterized?”

Instead ask:

* “What is this operation fundamentally doing?”
* “What concept do these cases share?”
* “Are these actually the same operation?”
* “Can the problem be represented differently so the distinction disappears?”

A good abstraction often has the paradoxical property that it addresses a broader conceptual problem while requiring **less machinery**.

A poor abstraction often claims to be general while introducing:

* flags;
* options;
* callbacks;
* special modes;
* subtype exceptions;
* branching logic;
* configuration switches.

If a generalized abstraction requires many flags to distinguish its users, reconsider whether those users actually belong to the same abstraction.

---

# 9. Abstraction Means Creating a Precise Semantic Level

Do not equate abstraction with:

* hiding information;
* putting code behind an interface;
* creating an object;
* creating a base class;
* polymorphism;
* dynamic dispatch;
* moving implementation into another module.

Those mechanisms may participate in an abstraction, but they are not themselves the essence of abstraction.

The purpose of abstraction is to identify fundamental concepts and express the problem at a semantic level where the behavior becomes clearer and more precise.

A good abstraction helps the programmer think in terms of the problem.

A bad abstraction makes the programmer think about the abstraction machinery.

If understanding the abstraction requires learning a large framework of internal mechanisms, ask whether it has actually simplified anything.

---

# 10. Good Abstractions Often Constrain Behavior

Do not assume flexibility is inherently desirable.

Every additional behavior that the system permits is another behavior a programmer may need to reason about.

Therefore, powerful abstractions often work by restricting what can happen.

Good APIs constrain callers toward correct usage.

Good types constrain representable states.

Good architecture constrains where mutation occurs.

Good execution models constrain when side effects can happen.

Good ownership constrains who can change state.

Good boundaries constrain which interactions are permitted.

An architecture where anything can call anything may appear flexible, but that flexibility expands the system's behavioral surface.

Every possible interaction becomes another interaction that may need to be considered during debugging, refactoring, testing, and reasoning.

Prefer deliberate constraints over arbitrary flexibility.

---

# 11. Keep Interfaces Small

The larger an interface becomes, the weaker its abstraction tends to become.

A broad interface exposes more operations, more relationships, and more states that callers must understand.

Do not expose capabilities simply because they may someday be useful.

Expose what callers actually need.

Restrict consequential operations.

Do not make internal mutation APIs universally available without a reason.

The smaller the permitted surface area, the fewer behaviors the system must support.

---

# 12. Make Execution Visible

A programmer should be able to understand what important work the system performs and when it performs it.

Prefer execution that can be read in a clear order.

All else being equal, prefer code that reads top-to-bottom over code whose real behavior is distributed across:

* callbacks;
* hooks;
* observers;
* implicit lifecycle methods;
* event propagation;
* middleware stacks;
* decorators;
* dependency injection side effects;
* deep call chains;
* hidden framework behavior.

The question “What happens next?” should have an obvious answer.

If understanding a feature requires reconstructing a distributed execution graph, reconsider the design.

---

# 13. Do Not Extract Functions Mechanically

Function extraction is not automatically an improvement.

A function boundary has a cost.

It may hide:

* state changes;
* dependencies;
* execution order;
* assumptions;
* error behavior;
* side effects.

If a block of code:

* has one caller;
* is conceptually part of that caller;
* participates closely in the caller's stateful execution;
* must be understood to understand the caller;

then moving it elsewhere may make the program harder to read.

In that case, extraction has not removed complexity.

It has merely moved or hidden complexity.

Do not refactor solely because a function is “too long.”

Function length is not a design goal by itself.

---

# 14. Prefer Locality for One-Use Stateful Operations

For single-use, stateful behavior, locality is often more valuable than small function size.

Keep related stateful operations together when doing so makes execution easier to understand.

A coherent large function can often be made readable through:

* scopes;
* comments;
* whitespace;
* clearly separated stages;
* descriptive local names.

Do not automatically convert every stage into another callable operation.

Callable operations increase the set of behaviors the system permits.

Keep important execution together when the operations form one coherent transition.

---

# 15. Inline One-Use Functions When That Improves Understanding

If a function:

* is only called once;
* primarily exists because of an earlier mechanical extraction;
* hides stateful execution;
* forces the reader to jump elsewhere to understand the caller;

consider inlining it.

Inlining is not regression.

Inlining can reduce indirection.

Inlining can reveal cause and effect.

Inlining can make state changes visible.

Inlining can make execution order obvious.

The objective is not maximizing reuse.

The objective is minimizing unnecessary mental navigation.

---

# 16. Large Functions Are Not Inherently Bad

Do not identify “large function” with “bad function.”

A large function may be appropriate when it contains one coherent operation whose stages need to be understood together.

A smaller function can be worse if its behavior is distributed among many indirectly connected helpers.

Judge a function by:

* conceptual coherence;
* visibility of state transitions;
* clarity of execution;
* number of responsibilities in the semantic sense;
* difficulty of reasoning about its effects.

Do not judge it primarily by line count.

---

# 17. Extract Pure Computation More Readily Than Stateful Execution

There is an important distinction between:

* pure computation; and
* stateful execution.

Pure functions are easier to compose, test, reason about, and reuse because their meaning depends primarily on their inputs and outputs.

When logic does not need to mutate external state, prefer making it pure where that clarifies the program.

It is often valuable to extract pure calculations from a stateful execution path.

At the same time, keep the major stateful transition visible.

A useful pattern is:

1. keep orchestration and consequential mutation localized;
2. extract self-contained calculations into pure functions;
3. return explicit values;
4. perform important side effects at controlled points.

Do not scatter the stateful operation merely because parts of it can technically become methods.

---

# 18. Minimize Branching

Branching multiplies execution paths.

Execution paths multiply reasoning burden.

Therefore, avoid unnecessary branching.

Reduce deeply nested conditionals.

Reduce the “area under ifs.”

Avoid architectures that repeatedly fork execution into many slightly different paths.

When behavior can use one consistent execution path, prefer that.

If multiple branches share meaningful behavior, look for a design in which the shared behavior occurs once.

Prefer predictable execution over saving tiny amounts of unnecessary work.

For many systems, doing a small harmless operation consistently is simpler than creating several conditional paths to avoid it.

---

# 19. Prefer Flags Within a Controlled Path Over Distributed Branches When Appropriate

When an operation is largely shared but a small piece is conditional, consider retaining one execution path and gating the genuinely conditional operation with explicit state.

Do not automatically split the entire process into multiple branches.

One path with a clearly defined condition may be easier to reason about than several separate flows whose behavior later diverges.

However, do not use flags to force semantically unrelated behaviors into one abstraction.

The principle is:

**Reduce accidental execution-path multiplication without falsely unifying distinct concepts.**

---

# 20. Prefer Controlled Execution Over Distributed Reactions

Architectures where behavior primarily emerges from many independent pieces reacting to each other can be difficult to reason about.

Be cautious with extensive use of:

* callbacks;
* observers;
* event subscribers;
* arbitrary event handlers;
* lifecycle hooks;
* implicit side effects;
* reactive chains.

These techniques have legitimate uses.

They are not forbidden.

But do not make important application invariants depend on reconstructing a long distributed reaction chain when the behavior can instead be expressed as a controlled state transition.

For important behavior, prefer an execution model where cause, transition, and effect can be inspected together.

---

# 21. Prefer Explicit State Transitions

Use explicit state-transition mechanisms where they simplify reasoning.

Examples include:

* reducers;
* ECS systems;
* state machines;
* transaction processors;
* centralized update systems;
* controlled command-processing stages.

These mechanisms are useful because they can make explicit:

1. what caused the operation;
2. what state existed before;
3. what transition occurs;
4. what state exists after;
5. what side effects result.

Prefer this structure for consequential application behavior.

The exact architectural pattern is not sacred.

The important property is **controlled, understandable transition ownership**.

---

# 22. Localize Major Behavior

Major behavioral side effects should occur at a small number of obvious callsites.

If possible, a reader should be able to inspect one controlled execution point and see the meaningful effects of an operation.

Do not distribute half of an operation across one system, another quarter through callbacks, and the remaining quarter through implicit observers merely to make each individual component smaller.

It is often better to leave complexity inside the individual system than to distribute behavioral complexity across interactions between systems.

Local complexity can be inspected.

Distributed complexity must be reconstructed.

Prefer inspectable complexity.

---

# 23. Encapsulate Behavior, Not Merely Representation

Traditional encapsulation often emphasizes hiding data representation behind objects.

Representation encapsulation can be valuable.

But behavioral encapsulation is often more important.

Behavioral encapsulation means restricting where consequential actions are allowed to happen.

Examples:

* one reducer owns a transition;
* one ECS system owns a mutation;
* one transaction processor commits an operation;
* one state machine owns transitions between states.

It can be acceptable to relax local representation encapsulation if doing so enables much stronger control over system behavior.

Do not protect a field so aggressively that the actual mutation semantics become distributed and obscure.

---

# 24. Treat Callability as Permission

Making a function callable from many places expands the behavior the system permits.

This matters especially for functions that:

* mutate important state;
* perform I/O;
* create or destroy resources;
* alter persistence;
* send messages;
* change ownership;
* perform irreversible actions;
* affect important invariants.

Do not expose consequential operations casually.

Ask:

**Who should be allowed to cause this behavior?**

Then structure the system so those are the actual permitted callsites.

A function being technically reusable is not necessarily a virtue.

---

# 25. Prefer Facts, Intentions, Requests, and Observations Over Arbitrary Cross-System Mutation

Rather than allowing every subsystem to directly perform consequential operations, prefer architectures where subsystems communicate:

* facts;
* intentions;
* requests;
* observations;
* commands;
* state descriptions.

A controlled owner can then resolve those inputs into actual state transitions.

This keeps important behavior centralized.

For example, prefer:

“System A reports that entity X should take damage”

over:

“System A reaches into several unrelated systems and directly mutates health, death state, scoring, effects, persistence, and achievements.”

The first creates a controlled resolution point.

The second distributes responsibility for invariants.

---

# 26. Localize Major Side Effects

Major side effects should not occur from arbitrary locations.

Consequential mutation becomes increasingly difficult to reason about as the number of possible callsites grows.

Prefer:

* a small number of deliberate mutation points;
* explicit operations;
* explicit transaction boundaries;
* controlled state transitions;
* identifiable owners.

If the same consequential behavior occurs from multiple locations, consider consolidating that behavior into one controlled execution point.

This does not mean all code must be centralized.

It means **responsibility for an important behavior should be centralized when practical**.

---

# 27. Uphold Each Invariant in as Few Places as Possible

An invariant becomes more fragile as the number of places responsible for maintaining it grows.

For every invariant, ask:

**Where is this invariant established and maintained?**

The answer should ideally identify one conceptual owner.

If five unrelated modules must all remember the same rule, the rule is fragile.

If one controlled transition point enforces the rule, the rule is stronger.

Avoid relying on programmer discipline across many callsites when architecture can reduce the number of enforcement sites.

---

# 28. Prefer Single Writers

When practical, important application state should have one conceptual writer.

Many systems may read the state.

Many systems may produce information relevant to changing the state.

But one conceptual owner should perform the mutation.

This greatly reduces the number of interactions that must be considered.

If ten systems can modify the same state, every invariant involving that state may need to be considered in all ten systems.

If one reducer, ECS system, state machine, transaction processor, or equivalent mechanism owns the transition, those invariants can be upheld in one place.

Single-writer design is particularly valuable for consequential or highly constrained state.

---

# 29. Distinguish Local and Global Invariants

A local invariant belongs entirely to one piece of data.

For example, a structure representing health may require:

* current health not to exceed maximum health;
* maximum health to satisfy some domain rule.

When an invariant belongs entirely to one data structure, encapsulating that structure's valid mutations may be appropriate.

Prefer enforcing a local invariant through a small number of controlled mutators or constructors.

A global invariant depends on relationships across broader application state.

Do not pretend a global invariant is local.

Do not distribute global invariant enforcement across every subsystem that might influence it.

Give system-wide invariants a system-wide owner.

The broader the invariant, the broader its legitimate enforcement point may need to be.

The constant rule is:

**Minimize the number of places responsible for keeping the invariant true.**

---

# 30. Make Invalid States Difficult or Impossible

Before adding validation for an invalid state, ask whether that state needs to be representable at all.

Before adding recovery for an error, ask whether the abstraction can eliminate the error.

Prefer defining problems out of existence.

Do not repeatedly detect a state that could instead be impossible to construct.

Do not treat all representable combinations as legitimate merely because the storage representation permits them.

Use:

* better data models;
* better constructors;
* better ownership;
* better transition rules;
* better types when appropriate;
* more constrained APIs;

to reduce invalid states.

The best validation logic is sometimes an abstraction in which the invalid case no longer exists.

---

# 31. Validate at Boundaries

Be defensive where your system meets something it does not control.

Examples include:

* user input;
* network messages;
* remote APIs;
* files;
* databases;
* serialized data;
* external processes;
* untrusted or externally produced data.

Validate uncertainty at the boundary.

Convert external input into a valid internal representation.

Establish the invariant.

Then allow internal code to rely on that invariant.

---

# 32. Validate Once, Then Trust the Internal Representation

Do not repeatedly validate the same invariant throughout the internal system unless there is a concrete reason that the invariant can legitimately become invalid.

Repeated validation has costs.

It:

* obscures which layer owns correctness;
* creates duplicated rules;
* implies malformed states are normal;
* increases branches;
* increases code volume;
* can cause different locations to disagree about validation semantics.

Once uncertain external data becomes valid internal state, internal code should generally trust the representation.

If internal code can freely violate the invariant, fix the ownership or transition design rather than scattering more checks everywhere.

---

# 33. Fail Clearly When Internal Invariants Are Violated

If an internal state violates an invariant that should already have been guaranteed, do not silently invent behavior.

Do not:

* guess missing values;
* substitute arbitrary defaults;
* silently repair corrupted relationships;
* make every internal function permissive;
* pretend malformed internal state is expected.

Fail clearly.

Expose the violation.

Make the broken assumption obvious.

Recovery logic should exist when the application intentionally defines recovery semantics.

Do not create accidental recovery semantics because failure feels uncomfortable.

---

# 34. Treat Errors as Values and Execution Flow

Errors are part of program behavior.

They are not secondary to “real” execution.

A program's error paths may constitute a large portion of its behavior.

That is acceptable.

Model meaningful failures deliberately.

Make error propagation understandable.

Keep failure semantics explicit.

Do not contort the design merely to minimize visible error-handling code.

An error is another possible result of execution and should be treated with the same architectural care as success.

---

# 35. Duplication and Indirection Are Both Costs

Do not follow “DRY” mechanically.

Duplication has a cost.

Indirection also has a cost.

A small amount of duplication can be cheaper than introducing a dependency between concepts that are not actually the same.

Likewise, consequential duplicated behavior can be dangerous when multiple copies must remain synchronized.

Evaluate the semantic nature of the duplication.

Ask:

**Is this duplicated knowledge, or merely similar syntax?**

This distinction matters.

---

# 36. Avoid Duplicating Knowledge

If two pieces of code encode the same rule and must change together when that rule changes, they probably represent duplicated knowledge.

This duplication is dangerous because the copies can drift apart.

Examples include:

* the same validation rule in five places;
* the same state transition implemented independently by several systems;
* the same business rule copied into multiple handlers;
* the same serialization assumption repeated throughout the system.

Where practical, give that knowledge one owner.

This is especially important for invariants and consequential behavior.

---

# 37. Similar Syntax Does Not Necessarily Mean Shared Knowledge

Two pieces of code may currently look similar while representing independent concepts.

Do not automatically unify them.

Prematurely combining independent concepts creates coupling.

If each concept could change independently for different reasons, a small amount of duplication may be preferable.

Code similarity is evidence worth examining.

It is not proof that an abstraction exists.

Do not create a common parent merely because two implementations currently contain similar lines.

---

# 38. Do Not Create Flag-Heavy Abstractions Merely to Remove Duplication

If extracting common behavior requires:

* mode flags;
* special-case switches;
* caller-specific branches;
* optional callbacks;
* many configuration parameters;

consider leaving the implementations separate.

An abstraction that constantly asks which caller invoked it may not represent a real shared concept.

Do not trade simple duplication for complicated dependency.

The objective is not “never duplicate.”

The objective is not “always abstract.”

The objective is to keep concepts, dependencies, invariants, and execution easy to reason about.

---

# 39. Consequential Behavior Should Usually Have One Implementation

Although small syntactic duplication may be acceptable, avoid duplicating major side-effecting operations merely to prevent a central control point.

If an operation must preserve important invariants, prefer one authoritative implementation.

For example, do not implement “delete account” independently in six UI flows.

Do not duplicate “apply transaction” logic across unrelated callsites.

Do not duplicate “transition state” logic when those copies must maintain the same invariants.

Centralize important knowledge.

Avoid centralizing unrelated syntax.

---

# 40. Prefer Deletability Over Hypothetical Extensibility

Do not attempt to predict every future requirement.

Future-proofing frequently creates present complexity for a future that never arrives.

Instead, prefer code that is:

* understandable;
* isolated;
* replaceable;
* easy to delete;
* easy to rewrite.

A future implementation can be built when the future requirement actually exists.

The current implementation should not become a generalized framework solely because it may need to change someday.

---

# 41. Make Code Replaceable

A good implementation need not support every future use.

It should instead avoid becoming so entangled with the rest of the system that replacing it is impossible.

This distinction is critical.

Do not design a plugin system because one implementation exists today and another might exist later.

Instead, keep the implementation's dependencies and responsibilities clear enough that it can be deleted and rebuilt later.

Replaceability is not the same as extensibility.

Prefer replaceability when the extension requirement is hypothetical.

---

# 42. Do Not Create Extension Points Without Extensions

Avoid:

* plugin systems with one plugin;
* interfaces with one implementation solely because a second implementation might someday exist;
* configuration values for constants that never vary;
* hook systems with no real hook users;
* generic strategies for one known strategy;
* compatibility layers nobody requires;
* frameworks justified mainly by imagined future flexibility.

Every extension point creates another concept and another permitted behavior.

Add extension mechanisms when actual requirements justify them.

---

# 43. Do Not Couple the Whole System to Implementation Details

Avoiding speculative interfaces does not mean hard-coding implementation specifics across the entire program.

The implementation should still be removable.

Keep implementation-specific details appropriately contained.

The important balance is:

* do not invent abstraction machinery merely for hypothetical replacement;
* do not spread implementation assumptions so widely that replacement becomes impossible.

Write the simplest current implementation with boundaries that make its responsibility obvious.

---

# 44. Test Behavior, Not Incidental Structure

Tests should primarily protect meaningful behavior and invariants.

Good tests protect things such as:

* a user-visible feature continuing to work;
* an important rule remaining true;
* a state transition producing the correct result;
* an important failure behaving correctly;
* a previously fixed bug not returning.

Tests should answer:

**What meaningful behavior would break if this test failed?**

If the answer is merely “the implementation changed shape,” reconsider the test.

---

# 45. Do Not Freeze Private Implementation Details Without a Reason

Avoid tests whose main purpose is to require that:

* a specific private helper is called;
* an internal method has a particular shape;
* an operation passes through an exact mocked sequence;
* private internal state has a specific representation;
* implementation steps occur in a specific order that callers do not observe.

Such tests constrain refactoring without necessarily protecting useful behavior.

Test through meaningful boundaries when practical.

The more internal structure a test freezes, the less freely the implementation can be simplified later.

---

# 46. Remember That Every Test Has a Cost

Tests are code.

Tests require maintenance.

Tests encode assumptions.

Tests constrain future changes.

Therefore, more tests are not automatically better.

Add tests where the behavioral protection justifies the maintenance cost.

Do not maximize test count.

Maximize confidence in meaningful behavior and invariants.

A small number of high-value behavioral tests may be more useful than a large suite of brittle implementation-detail tests.

---

# 47. Refactoring Rules

When refactoring, do not assume the objective is to create:

* smaller functions;
* more classes;
* more interfaces;
* fewer duplicated lines;
* more generic abstractions;
* more design patterns.

The objective is to reduce conceptual complexity.

A successful refactor should ideally produce one or more of the following:

* fewer concepts;
* fewer states;
* fewer mutation sites;
* fewer execution paths;
* fewer dependencies;
* fewer invariant enforcement points;
* clearer ownership;
* clearer state transitions;
* more visible execution;
* fewer hidden side effects;
* more obvious data relationships;
* easier replacement of isolated implementations.

If a refactor increases the number of concepts without removing greater complexity elsewhere, reconsider it.

---

# 48. Architecture Review Rules

When reviewing an architecture, explicitly ask:

1. Where does important state live?
2. Who can mutate it?
3. How many writers exist?
4. What are the important invariants?
5. Where is each invariant enforced?
6. How many callsites can trigger consequential behavior?
7. Can I determine the execution order?
8. Can I determine why a side effect occurred?
9. Are callbacks or events obscuring cause and effect?
10. Are there extension points without real extensions?
11. Are interfaces larger than necessary?
12. Are unrelated concepts coupled because they currently look similar?
13. Are we modeling the actual data and its relationships?
14. Can invalid states be removed?
15. Are external inputs validated once at clear boundaries?
16. Are tests protecting behavior rather than implementation shape?
17. Could any abstraction be deleted without making the underlying problem harder?
18. Could the implementation be easier to delete instead of more extensible?
19. Is complexity localized or distributed?
20. Does the architecture make important behavior more obvious?

Use these questions repeatedly.

Do not accept architectural sophistication as evidence of quality.

---

# 49. Rules for Adding a New Abstraction

Before creating an abstraction, determine which concrete complexity it removes.

Do not create an abstraction merely because code looks repetitive.

A new abstraction should ideally:

* reveal a real semantic concept;
* remove special cases;
* reduce the number of states;
* reduce the number of permitted interactions;
* centralize important knowledge;
* clarify ownership;
* make execution easier to understand.

Be suspicious if the proposed abstraction instead adds:

* configuration;
* callbacks;
* flags;
* extension points;
* additional lifecycle phases;
* extra interfaces;
* extra dependency injection;
* more branching;
* more modes.

An abstraction that needs substantial machinery to unify several cases may be weaker than leaving those cases independent.

---

# 50. Rules for Adding Dependencies

A dependency is not free.

Every dependency adds concepts, behavior, assumptions, update considerations, and potential interactions.

Before adding a dependency, ask whether the underlying problem warrants the additional conceptual surface.

Do not add a framework because it makes one small operation convenient.

Do not replace transparent local code with opaque library behavior without considering the reasoning cost.

Dependencies are justified when they remove more complexity than they introduce.

---

# 51. Rules for Events, Hooks, and Reactive Systems

Events, hooks, observers, and reactive systems are permitted.

They are not preferred by default for consequential application behavior.

Use them when their semantics genuinely match the problem.

Do not use them merely to avoid direct orchestration.

For every important event chain, ask:

* Can a programmer identify the producer?
* Can a programmer identify the consumers?
* Is ordering defined?
* Are side effects obvious?
* Can important invariants be upheld centrally?
* Can the cause of a behavior be reconstructed easily?

If not, consider replacing distributed reaction with controlled execution.

---

# 52. Rules for Side Effects

Classify significant side effects as architectural concerns.

Do not hide them casually.

Important side effects should happen in identifiable places.

Prefer code where:

* pure computation is pure;
* state transitions are explicit;
* I/O is visible;
* mutations are deliberate;
* consequential operations have clear owners.

Do not allow a harmless-looking getter, constructor, callback registration, or helper invocation to perform surprising consequential work without a compelling reason.

Unexpected side effects increase the number of behaviors a programmer must consider.

---

# 53. Rules for State Ownership

Every important piece of mutable state should have understandable ownership.

Ask:

* Who owns this state?
* Who may read it?
* Who may write it?
* What invariants involve it?
* Where are its transitions defined?

If the answer to “who may write it?” is “almost anything,” consider the design suspect.

If ownership cannot be explained succinctly, the architecture may be too distributed.

Prefer one conceptual owner even when multiple implementation functions participate internally.

---

# 54. Rules for Internal APIs

Internal APIs are still APIs.

Every callable operation expands the internal behavior surface.

Keep internal APIs as narrow as practical.

Do not expose mutators merely because another subsystem might eventually need them.

Do not make all state externally writable for convenience.

Prefer expressing higher-level intentions rather than exposing low-level mutation primitives when doing so protects invariants.

A narrow internal API can be more valuable than rigid private-field encapsulation.

---

# 55. Rules for Error Handling

Handle errors deliberately.

At external boundaries:

* expect malformed or unexpected input;
* validate it;
* return meaningful errors;
* convert it into trusted internal representations.

Inside trusted boundaries:

* rely on established invariants;
* fail clearly when supposedly impossible conditions occur;
* do not silently continue with invented defaults.

Do not turn every function into a defensive parser for internal state.

That approach weakens the meaning of your invariants.

---

# 56. Rules for Performance Decisions

Do not introduce execution complexity merely to avoid small amounts of harmless work.

A consistent path with predictable worst-case behavior may be preferable to several optimized branches.

Do not prematurely fragment execution to optimize cases that have not been shown to matter.

Performance matters, but reason about it in terms of actual requirements.

When optimization is necessary, preserve clear ownership and execution visibility where possible.

---

# 57. Rules for Code Review

When reviewing generated or existing code, do not focus only on localized style.

Look for systemic complexity.

Flag designs where:

* many modules mutate the same state;
* events trigger consequential behavior invisibly;
* invariants are duplicated;
* validation is repeated everywhere;
* extension points exist without users;
* a generic abstraction contains many modes;
* helpers hide the main execution path;
* mocks freeze incidental implementation structure;
* similar syntax has been prematurely unified;
* deep call chains obscure what happens;
* important side effects occur from arbitrary places.

Conversely, do not criticize code merely because:

* a function is long;
* a reducer is large;
* a system contains explicit procedural stages;
* a small amount of syntax is duplicated;
* a local implementation is concrete;
* stateful orchestration is kept in one place.

Judge the reasoning burden of the whole design.

---

# 58. Rules for Generating New Code

When implementing a feature:

1. Understand the data.
2. Understand the relationships.
3. Identify the transformations.
4. Identify the important invariants.
5. Identify the owner of each important state transition.
6. Keep major side effects controlled.
7. Use as few execution paths as practical.
8. Validate external uncertainty at the boundary.
9. Trust valid internal representations.
10. Extract pure computation when useful.
11. Keep one-use stateful execution visible.
12. Avoid speculative flexibility.
13. Add abstractions only when they simplify the semantic model.
14. Test meaningful behavior.

Do not begin by inventing a framework around the feature.

Implement the problem that exists.

---

# 59. Rules for Modifying Existing Code

When changing an existing system, preserve established architecture unless there is a meaningful reason to improve it.

Do not opportunistically introduce unrelated abstractions.

Do not “clean up” code by splitting coherent execution into many helpers solely to satisfy generic style preferences.

Do not convert direct state transitions into event-driven interactions without a semantic reason.

Do not add interfaces, factories, or dependency-injection layers just because they are common patterns.

If the existing code has a clear single-writer model, preserve it.

If the existing code deliberately localizes behavior, preserve that locality.

If the change exposes a deeper semantic problem, fix the underlying concept when doing so is justified.

---

# 60. ECS-Specific Rules

When working in an Entity Component System architecture, apply all general rules above and additionally follow these rules.

## 60.1 Components Should Be Data

Components should primarily represent data.

Keep behavior out of components where practical.

Accessors and setters are acceptable when they preserve local data invariants.

Do not turn components into miniature behavioral objects that communicate extensively with other components.

The system should not depend on distributed component behavior to understand execution.

## 60.2 Keep Entity Structure Flat

Prefer flat entity composition.

Do not create elaborate nested entity structures merely to model hierarchy.

When hierarchy is necessary, prefer components containing references to other entities.

Keep relationships explicit in data.

## 60.3 Localize Major Behavior in Systems

Major application behavior should live in the relevant systems and their controlled callsites.

Do not distribute consequential behavior across many components.

Do not make components react to each other through hidden behavioral interactions.

The system should be the place where the transition can be understood.

## 60.4 Leave Complexity Inside the System Rather Than Between Systems

A large ECS system may be preferable to an architecture where behavior is scattered among many interacting components and callbacks.

Do not split a coherent system merely to make each individual system appear smaller.

Keep the behavior inspectable.

It is better for complexity to be visible inside the system than hidden in relationships between many systems.

---

# 61. Priority Rules When Principles Conflict

These principles can occasionally pull in different directions.

When they conflict, prioritize approximately as follows:

1. **Correctness and preservation of required behavior.**
2. **Clear ownership of important state and invariants.**
3. **Visible and understandable execution.**
4. **Reduction of possible states and behavioral interactions.**
5. **Accurate semantic modeling of data and relationships.**
6. **Localization of consequential side effects.**
7. **Reduction of unnecessary dependencies and indirection.**
8. **Deletability and replaceability.**
9. **Code reuse and elimination of syntax duplication.**
10. **Aesthetic preferences such as function size or stylistic symmetry.**

Do not sacrifice higher-priority properties merely to improve lower-priority ones.

In particular:

* do not obscure execution to reduce line count;
* do not distribute mutation to make functions smaller;
* do not create coupling merely to remove duplicated syntax;
* do not create hypothetical extensibility at the cost of current clarity;
* do not weaken ownership merely to expose a more convenient API.

---

# 62. Default Biases

When there is no strong evidence favoring another approach, use these defaults:

Prefer explicit over implicit.

Prefer direct over indirect.

Prefer local over distributed.

Prefer controlled mutation over arbitrary mutation.

Prefer a small behavioral surface over broad flexibility.

Prefer one owner over many owners.

Prefer one enforcement point over repeated enforcement.

Prefer one coherent execution path over many subtly different paths.

Prefer semantic abstractions over structural abstractions.

Prefer pure functions for pure computation.

Prefer localized stateful orchestration for stateful behavior.

Prefer boundary validation over pervasive defensive checking.

Prefer replaceability over speculative extensibility.

Prefer behavior tests over implementation-detail tests.

Prefer a little harmless duplication over an unnecessary dependency.

Prefer centralizing duplicated knowledge over centralizing merely similar syntax.

Prefer code that tells you what happens over code that makes you discover what happens.

---

# 63. Anti-Goals

Do not optimize the codebase for any of the following as independent goals:

* minimum line count;
* maximum abstraction;
* maximum modularity;
* maximum number of small functions;
* maximum interface usage;
* maximum dependency injection;
* zero duplicated syntax;
* maximum reuse;
* maximum genericity;
* maximum extensibility;
* maximum configurability;
* maximum encapsulation of representation;
* maximum test count;
* maximum number of design patterns.

Any of these properties may be useful in a particular context.

None is intrinsically desirable.

Use them only when they reduce the actual complexity of the system or protect meaningful correctness.

---

# 64. Signs That the Design Is Becoming Too Complex

Treat the following as warning signs:

* It is difficult to determine what calls what.
* Important behavior can originate from many places.
* Debugging requires tracing several event chains.
* A piece of state has many writers.
* The same invariant is checked repeatedly throughout the application.
* Helpers need many parameters because they serve unrelated callers.
* An abstraction accumulates mode flags.
* A framework exists mostly to support imagined future use.
* A simple change requires touching many independent layers.
* Internal functions defensively validate states that should be impossible.
* Tests fail frequently because private helper structure changes.
* A reader must jump through many tiny functions to understand one operation.
* Nobody can clearly identify the owner of a state transition.
* A feature's behavior emerges from interactions rather than being expressed directly.
* An interface exposes many operations that most callers should never use.
* A subsystem cannot be deleted because implementation assumptions are spread everywhere.
* Every system knows about every other system.

When these signs appear, do not merely add documentation.

First ask whether the design itself can be made simpler.

---

# 65. Signs of a Strong Design

Prefer designs where:

* important data structures are obvious;
* important relationships are explicit;
* state has clear ownership;
* mutations happen at deliberate points;
* execution can be followed in a comprehensible order;
* important behavior has obvious callsites;
* invariants have identifiable owners;
* invalid states are difficult to create;
* external uncertainty is handled at boundaries;
* internal code can rely on established guarantees;
* abstractions correspond to real concepts;
* extension mechanisms exist because real extensions exist;
* implementations can be replaced without having been prematurely generalized;
* tests protect observable behavior and important invariants;
* the number of possible interactions is intentionally constrained.

The objective is not to make the code look minimal.

The objective is to make the **space of possible program behavior understandable**.

---

# 66. Final Decision Rule

Whenever you are uncertain between two implementations, do not ask only:

“Which is cleaner?”

Ask:

* Which requires understanding fewer concepts?
* Which permits fewer surprising behaviors?
* Which has clearer ownership?
* Which has fewer mutation sites?
* Which keeps important side effects visible?
* Which makes execution easier to follow?
* Which gives invariants fewer enforcement points?
* Which better reflects the data and its relationships?
* Which creates fewer dependencies?
* Which introduces fewer hypothetical capabilities?
* Which is easier to delete or replace?
* Which would be easier to debug when something goes wrong?

Choose the implementation that makes the program's behavior easier to know.

That is the governing principle.

---

# 67. Compact Instruction for the Agent

When writing code under these rules:

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
