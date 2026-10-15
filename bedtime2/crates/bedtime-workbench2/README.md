# bedtime-workbench2

The first workbench service is typed command registration and controlled execution.
Panels, document/history services, workspaces and settings are not implemented yet.
`WorkbenchPlugin` runs headlessly without installing `DesktopPlugin` or a renderer.

## Register a command

```rust,ignore
struct SaveDocument;
impl CommandDefinition for SaveDocument {
    const KEY: &'static str = "example.save_document";
    const LABEL: &'static str = "Save Document";
}

let mut workbench = Workbench::new(app.world_mut());
let save = workbench.register_command::<SaveDocument, _>(save_document)?;
```

The trailing `_` infers Bevy's system-parameter marker. Handlers are ordinary
Bevy systems, including exclusive systems, with `In<CommandInvocation>` input and
`CommandResult` (`Result<(), String>`) output. Their resources and queries are
injected normally by Bevy.

Registration decorates Bevy's registered-system entity with `CommandInfo` and a
required `ActionEnabled(true)` component. That entity is the returned runtime
handle. Metadata and lookup live in ECS; there is no parallel command registry.

`CommandDefinition::KEY` is a stable, namespaced string. Keys require at least
two dot-separated segments containing ASCII letters, digits, `_` or `-`.
Registration rejects duplicate keys or duplicate definition types before installing
a handler. `LABEL` provides initial human-readable command metadata.

Use `workbench.command::<SaveDocument>()` for typed lookup. `CommandInfo::key()`
returns a `CommandKey`, which serializes as a plain string. Deserialize that key
and use `command_by_key(key.as_str())` to resolve a current entity after restart.
Unknown keys return `None`; saved shortcut storage and missing-plugin retention
policy remain future work. Runtime `Entity` and `TypeId` values are never persisted.

## Present and invoke a command

A desktop `MenuItem` references the returned entity through `Activates(save)`.
Menu labels and shortcuts belong to their placements. `ActionEnabled` belongs to
the shared action and affects all its menu presentations and dispatcher admission.
Removing a menu does not remove the command; despawning the command detaches its
presentation references.

Native and UI activations targeting registered commands are collected in
`PostUpdate`, in `CommandSystems::Ingress`, before dispatch. Order UI drawing
before this set to admit its actions in the same frame. Other action entities
remain available to application consumers. Activations retain their source and target. Application systems can also
submit through the durable `CommandRequests` resource:

```rust,ignore
requests.invoke(save);
requests.submit(CommandInvocation { command: save, source: Some(panel) });
```

The sole dispatcher runs in `PostUpdate`, in `CommandSystems::Dispatch`. It
processes the current FIFO batch, checks that each command still exists and is
enabled, executes the handler, and emits `CommandFinished` with its result.
Handler errors do not prevent later requests from executing. Command execution
is never a relationship hook or an immediate native callback.

Requests enqueued by a handler remain queued until the next dispatch pass. This
prevents recursive execution and leaves the state transition boundary explicit.
Invocations carry source observations; handlers must capture any additional
operation context they need rather than resolving it from later UI focus.

Results are ordinary Bevy messages, intended for consumers to read promptly.
The initial system has no asynchronous command barriers, durable result receipts,
argument serialization or shortcut editor.

```sh
cargo test --locked -p bedtime-workbench2 --lib
cargo run --locked -p bed2
```
