# bedtime-work

Application- or entity-owned background tasks with cooperative cancellation
and shutdown. Depends only on Bevy.

```rust,ignore
app.add_plugins(WorkPlugin);

let task = world.resource_mut::<ManagedWork>().spawn_task(Some(panel), "Search", |cancel| {
    if cancel.wait(Duration::from_secs(2)) { return None; }
    Some(compute_result())
})?;
// Later: task.try_recv(), task.status(), task.cancel().
```

- Tasks run on dedicated threads and return values through `ManagedTask`.
  They cannot access the World.
- Removing an optional owner entity cancels its tasks on the next update. `spawn_task` is for disposable calculations:
  cancellation can suppress its returned value.
- `ManagedTask` reports Running, Cancelling, Cancelled, Finished and Failed;
  cancellation is cooperative (`Cancellation::is_cancelled`, `wait`).
- `ManagedWork::shutdown` (also on `AppExit`) cancels everything and refuses
  new work; `is_finished` reports completion. Dropping the World alone
  is a fallback, not an orderly shutdown. Tasks must return after
  cancellation.
- Polling runs in `Last` (`WorkSystems::Poll`).

```sh
cargo test -p bedtime-work
```
