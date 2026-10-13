# Plain editor application

This application launches through `bed_app::Application` and adds the default
`EditorPlugin`. Two text panels share one document, while each keeps its own
caret and scroll state.
The foundation owns document requests, local saves and shared undo/redo. The
application uses the SDK's standard frontend and settings defaults without a
workspace, Git, LSP, remote storage or syntax services.

Run the desktop application:

```sh
cargo run --offline -p bed-example-editor
```

The example creates its document in `Startup`, opens two panels with
`bed_core::open_document_panel`, and splits the second into another area. Use
the core menu or the usual save, find, clipboard and undo shortcuts. An unnamed
document asks for a destination when saved.

Verification modes use an isolated temporary file and remove it when finished:

```sh
cargo run --offline -p bed-example-editor -- --headless-smoke
cargo run --offline -p bed-example-editor -- --smoke-test
cargo test --offline -p bed-example-editor
```

The headless check exercises both presentations, revision-bound edits,
undo/redo, a save receipt and saved bytes, and restoration of a layout with
independent view positions. The native check runs the same assertions after
rendering and exits after the restored panels have drawn several frames.

`bed-plugin-editor` and `bed-editor-widgets` offer a `language-services` feature for the
existing code editor, diagnostics, language-server tools and syntax decoration.
bEd selects this feature explicitly; this example uses the plain defaults.
