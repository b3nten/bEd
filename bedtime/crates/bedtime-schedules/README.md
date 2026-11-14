# bedtime-schedules

A Bevy app's schedules, read from the running app and shown in a panel: which
systems run, in what order and in which sets, where the `ApplyDeferred` sync
points are, and how data moves between systems through the components and
resources they read and write. For debugging bed, or any app built with it.

```rust,ignore
app.add_plugins(SchedulesPlugin);
```

That adds **Schedules** to the View menu (`schedules.show`). The plugin asks
nothing of the app's other plugins: their systems are read from the schedules
they were added to.

- **Views.** Graph (one schedule: sets as boxes, ordering edges, sync points,
  ambiguities), Lanes (the whole frame: a row per component or resource, a
  mark for each read and write, a line from each write to the reads it
  feeds), Arcs, Timeline, Flame and List. Click a system for its sets, run
  conditions, queries, ordering and ambiguities; click a component for
  everything that touches it.
- **Reading** (`ScheduleExtractPlugin`, `snapshot_now`) needs no UI: build
  without the `panel` feature for only that. A build pass on every schedule
  records each system's name, conditions, ordering and the access the
  schedule keeps for it; an `AppSnapshot` resource is rebuilt when a schedule
  is. Systems aren't initialised again, so their state is left alone.
- **Timings** (`profile`, off by default) show in the Timeline: how long each
  system took over the last 60 frames, and on which thread. It turns on
  Bevy's `trace` spans, which cost a little everywhere, and the app installs
  the layer that listens to them:

  ```rust,ignore
  DefaultPlugins.set(bevy::log::LogPlugin {
      custom_layer: bedtime_schedules::profile::layer,
      ..default()
  })
  ```

  bed does with `cargo run -p bed --features profile`.
- **Colours** are the theme's. Sets, schedules and components each get a hue
  hashed from their name.

Only the main world's schedules are read: not the render app's. A panel's
view (what's selected, hidden, searched for) isn't saved, only that it's open.

Adapted from [bevygraph](https://github.com/tbillington/bevy_debug_view)
(`licenses/bevygraph`), ported to Bevy 0.20.
