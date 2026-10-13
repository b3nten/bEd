# Model application

An application launched through `bed_launch::LaunchBuilder` with one custom panel
plugin. Two panels share a typed graph document. Moving a node commits one semantic change;
undo/redo updates both panels through the common document commands.

```sh
cargo run --locked -p bed-example-model
cargo run --locked -p bed-example-model -- --smoke-test
```

`DocumentModel::apply` returns an inverse change. Core applies changes to a clone
before committing contents, revision and history together, so rejected edits
leave the live model untouched. `DocumentBinding` connects a panel to its model
without a text-editor dependency. The panel reads contents and submits commands;
the registered command handler commits the edit after drawing.

Core's Save As command supplies a destination for the model's encoded contents.
An application with its own persistence can call `mark_model_saved` with the
exact revision it persisted. A stale acknowledgement leaves newer edits dirty.
This example starts a new model on each launch; applications choose how to load
their own model format and retain document identities across restarts.

The native smoke exercises shared views, edits, undo/redo, closing one view,
retaining the document and rejecting a save without a destination.

The native `--smoke-test` path also captures the real window and a visible panel
after presentations, and fails if the application exits before completing its
assertions. Capture output is written under `target/`.
