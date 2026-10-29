# bedtime-persistence

Saving a Bevy app's state between runs: stores, one world snapshot, and
resources saved as their own files. Needs no other bedtime crate.

```rust,ignore
use bedtime_persistence::*;

struct GlobalStore;
impl Store for GlobalStore {}

#[derive(Component, Reflect)]
#[reflect(Component, Persist)]
struct Note(String);

#[derive(Resource, Default, Serialize, Deserialize, Persisted)]
#[persisted(key = "recent")]
struct Recent(Vec<PathBuf>);

let config = config_directory("myapp", Some("MYAPP_CONFIG_DIR")).unwrap();
app.add_plugins(PersistencePlugin)
    .register_type::<Note>()
    .init_resource::<Recent>()
    .add_store::<GlobalStore>(DirectoryStore::new(config))
    .persist_world::<GlobalStore>()       // world.ron
    .persist::<Recent, GlobalStore>();    // recent.ron
```

- **Stores** are places. A `Store` is a marker type; `add_store::<S>(backend)`
  gives it a `StoreBackend` and turns saving on. A store never added isn't
  saved. `DirectoryStore` writes one `<entry>.ron` per entry, atomically;
  `MemoryStore` keeps entries in memory (tests, or apps that save elsewhere).
- **The world snapshot.** Components and resources marked
  `#[reflect(Persist)]`, entities included, are saved together as one reflected
  snapshot, in the one store `persist_world::<S>()` names. Both sides of every
  relationship are written back exactly, the way Bevy scenes work. Entity
  fields need Bevy's `#[entities]` mapping, and every entity a saved value
  refers to must be saved too. Asset handles can't be saved.
- **Persisted resources.** `persist::<T, S>()` saves a serde resource as its
  own entry, named by `#[persisted(key = "...")]` so renaming the type doesn't
  lose it. It's loaded at startup, written when it changes, and fails on its
  own. `read_persisted` / `write_persisted` use the same entry without an App
  (to decide how to build one, say).
- **When.** Everything is read in `PreStartup` (`StoreSystems::Load`) and saved
  in `Last` (`StoreSystems::Save`) about once a second while it changes, and
  when the app exits. Unchanged data isn't rewritten.
- **Failure.** An entry whose saved data can't be read or restored is never
  written, so a damaged file isn't overwritten. Failures are `StoreError`
  messages (`Display` gives a sentence for the user); `Stores::error::<S>()`
  says why something in a store isn't being saved.
- **Hooks**, for plugins whose data needs more than reflection:
  `on_capture` edits a snapshot before it's saved, `check_restore` refuses one
  before anything is written, and `on_restore` fixes things up afterwards (an
  error undoes the restore's entities).
- `world.persistence().capture()` and `world.persistence_mut().restore(text)`
  capture and restore the snapshot directly; `snapshot` has the pieces for
  saving part of a world the same way (workbench's layout templates use it).
- `config_directory(app, override_variable)`: the override if set, else
  `$XDG_CONFIG_HOME/<app>`, else `~/.config/<app>`.

```sh
cargo test -p bedtime-persistence
```
