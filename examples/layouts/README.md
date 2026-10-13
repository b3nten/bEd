# Layout app

A desktop app launched through `bed_app::Application`, with `bed-plugin-layouts`
and an ordinary Bevy counter plugin. The foundation supplies windowing, ImGui,
theming, icons, layout, shared document
editing/history, and registry menus. The app does not add code-editor, terminal,
language-server, or content-viewer plugins.

The example configures three numbered layouts. Switching keeps each layout's
panel entities, counter drafts, geometry, and focus alive. `LayoutDemoPlugin`
chooses its supported slots; `bed-plugin-layouts` itself supports any positive numbered
ID or an application-defined `LayoutKey` enum.

Run `cargo run -p bed-example-layouts`. The optional `--layout path.json` selects its
saved file (default: `layout-app.json`). The registry menu offers Layout 1–3,
New counter, Save all layouts, and Reset current layout.

Persistence belongs to the application. Its `DemoLayouts` integration saves
before switching and persists the successful destination, allowing the layout
service to roll back if persistence fails. The JSON file contains the active slot
and snapshots for all visited layouts. Restart restores the active slot and
loads the others when selected. Old single-layout files remain readable and
convert to the numbered wrapper on the next save.

Run `cargo test -p bed-example-layouts` for headless save/restart and migration checks.
Run `cargo run -p bed-example-layouts -- --smoke-test` for a native proof of command
execution, splitting, retained slot entities/state/focus, disk persistence, and
restoration into a fresh app. Add `--capture-frame /tmp/layout.ppm` to capture
the composed window before exit. The smoke uses its own temporary saved file
unless `--layout` supplies a path.
