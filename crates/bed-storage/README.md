# bed-storage

Plugins contribute JSON snapshots through `FeatureStateContribution` and announce
changes through `FeatureStateChanged`. Contributors own serialization; the host
chooses an opaque `StateScope` and supplies a `StateStore` implementation.

`collect_snapshot` merges installed contributions into a previous snapshot so an
absent plugin's data survives. `restore_snapshot` runs before panel creation and
passes `None` to installed plugins with no saved state.

Core installs `Storage` with `MemoryStateStore`. Collecting snapshots causes no
persistent writes. Project files and document history use their existing file
and document APIs; they are separate from plugin-state persistence.
