# bedtime-work

Background work owned by the app or by entities, with cooperative
cancellation and an orderly shutdown. Depends only on Bevy.

```rust,ignore
app.add_plugins(WorkPlugin);

let task = world.resource_mut::<ManagedWork>().spawn_task(Some(panel), "Search", |cancel| {
    if cancel.wait(Duration::from_secs(2)) { return None; }
    Some(compute_result())
})?;
// Later: task.try_recv(), task.status(), task.cancel().
```

- Tasks run on their own threads and hand back a value through a
  `ManagedTask`; they never touch the World.
- An optional owner entity gives work a shorter lifetime: its removal cancels
  the work on the next update. `spawn_task` is for disposable calculations:
  cancellation can suppress its returned value.
- `ManagedTask` reports Running, Cancelling, Cancelled, Finished and Failed;
  cancellation is cooperative (`Cancellation::is_cancelled`, `wait`).
- `ManagedWork::shutdown` (also on `AppExit`) cancels everything and refuses
  new work; `is_finished` says when it's all stopped. Dropping the World alone
  is a fallback, not an orderly shutdown. Tasks must return after
  cancellation.
- Polling runs in `Last` (`WorkSystems::Poll`).

```sh
cargo test -p bedtime-work
```
