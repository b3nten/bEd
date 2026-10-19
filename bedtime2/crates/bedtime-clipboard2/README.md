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
text or files, or is empty after `clear()`; writing another format replaces it.
File lists contain references, not file contents, and this crate performs no
filesystem operations.
`read_text_if_present()` returns `Ok(None)` for an absent text representation,
including a file-only clipboard, while native access/conversion errors remain
errors. `read_text()` retains its required-text API and reports absence as
`io::ErrorKind::NotFound`.
`clear()` removes all representations and uses the native clear operation, not
`write_files(&[])` (which is not supported by every native backend). After clearing,
optional text reads return `None`; required text/file reads report `NotFound`.
Native clear failures propagate without advancing the shared revision.

`revision()` counts successful text/file writes and clears through all cloned
handles; reads and external OS writes do not advance it. `read_files_snapshot()` captures files
and that local generation under one lock. `matches_files_snapshot()` checks the
same shared service, unchanged local generation and current file contents. Local
same-payload rewrites invalidate a snapshot, and native read errors remain errors.
In memory mode a match proves no intervening write. In native mode it cannot detect
an external same-payload replacement, or replacement and restoration between reads:
arboard's public API exposes no native change counter (including on macOS).
Consequently neither revision nor a matching native snapshot alone authorizes a
destructive move. Pending cut state, native ownership evidence where available,
revalidation and filesystem execution belong to the application. This crate does
not publish native cut intent or interpret an external file list as a cut.

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
