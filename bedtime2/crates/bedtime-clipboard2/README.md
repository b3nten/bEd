# bedtime-clipboard2

Independent text and file-list clipboard support for Bevy. No workbench, window,
renderer or UI is required. The API uses a shared `Clipboard` resource:

```rust,ignore
app.add_plugins(ClipboardPlugin);

fn copy(mut clipboard: ResMut<Clipboard>) {
    clipboard.write_text("Hello").unwrap();
    clipboard.write_files(&[PathBuf::from("/project/readme.md")]).unwrap();
}
```

The default build uses an in-process clipboard. Enable `system_clipboard` for
native desktop access. A native initialization failure selects memory mode and
remains observable through `initialization_error()`. `mode()` reports `System`
or `Memory`; `Clipboard::memory()` explicitly selects local copy/paste even in a
native build. The native workbench feature enables OS access for normal apps.

Reads/writes on an initialized native backend return their errors. They do not
silently return old local contents. Memory mode holds one current payload, either
text or files; writing another format replaces it. File lists contain references,
not file contents, and this crate performs no filesystem operations.

Cloned handles share one backend and mutex. UI2's egui adapter and application
systems therefore access the same clipboard without removing its resource from
the world or opening multiple native handles. `ClipboardPlugin` and UI2 preserve
an application-provided resource.

The resource/plugin design is adapted from Bevy 0.20's `bevy_clipboard`, under
MIT OR Apache-2.0. The accompanying license files preserve its attribution.
This version adds file lists, shared backend handles and native initialization
fallback. Browser and mobile targets currently use local copy/paste; this crate
does not implement browser clipboard permissions or asynchronous browser reads.

```sh
cargo test --offline -p bedtime-clipboard2 --lib
cargo test --offline -p bedtime-clipboard2 --lib --features system_clipboard
```
