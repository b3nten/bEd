# Layout app

A desktop app built from `bed-desktop`, `bed-workbench` and an ordinary Bevy
feature plugin. It has no editor, terminal, language-server or content-viewer
dependencies. Counter values are components on live panel entities. Commands
increment counters, create panels and save layouts; the frontend emits split and
close requests.

Run `cargo run -p bed-layout-app`. The optional `--layout path.json` selects its
saved layout file (the default is `layout-app.json`). Use the top menu to add
counters, save or reset the layout.

Run `cargo run -p bed-layout-app -- --smoke-test` for a native check of command
execution, splitting, disk persistence and fresh panel identities after restore.
Add `--capture-frame /tmp/layout.ppm` to capture the composed window before the
smoke exits.
