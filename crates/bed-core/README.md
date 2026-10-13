# bed-core

`BedCorePlugin` composes the desktop window, Bevy renderer, Dear ImGui, fonts,
theming, icons, workbench panels and shared documents. Install it on a fresh
`App`, then add the panels your application needs. The application owns its
configuration, startup choices and persistence.

```rust,no_run
use bed_core::{BedCorePlugin, bevy::prelude::*};
use bed_plugin_terminal::TerminalPlugin;

fn main() {
    App::new()
        .add_plugins(BedCorePlugin::default())
        .add_plugins(TerminalPlugin)
        .run();
}
```

The shared contracts are organized in [`bed-api`](../bed-api/README.md),
re-exported as `bed_core::api`. Features contribute panels and commands through `bed_core::api::workbench::register`.
Each contribution supplies its label and handler once. `panel_icon`,
`command_icon`, `menu` and `toolbar` attach presentation metadata to those
definitions. `CommandPresentations` supplies current visibility, enabled state
and optional label overrides. ImGui menus, native menus/toolbars and application
adapters resolve those facts with `CommandDefinition::presentation`.

The default frontend draws the workbench, command menu and contributed status
items. A host may supply `DesktopUi` before installing core to draw its own
frontend. `HostRequestMode::Application` transfers document request and
native presentation ownership to an application adapter, as used by bEd.
Set that mode before adding core so command and shortcut registration are also
delegated to the application.

## Documents and editing

Core installs an empty `DocumentSession` for canonical text and byte documents,
revision-bound edits, multiple views and undo/redo. It polls document services,
publishes document snapshots and resolves requests after drawing. Add
`bed_plugin_editor::EditorPlugin` for the existing text and hex widgets. Its default
build does not install LSP, highlighting, Git or SSH services; bEd explicitly
enables its `language-services` feature.

`open_file` selects a registered viewer and binds its document or local file
before running the panel factory. `open_document_panel` creates another view of
an existing document. Saved file-backed panel records include the path and
content kind so layout restore can recreate their bindings.
Untitled panels reuse their validated document identity within the same live
session. Persistence across restarts requires the application's document
serializer; a missing identity produces an unavailable panel with its saved
record retained rather than replacing the document with an empty buffer.

For asynchronous producers, capture a `HostRequest` at emission and write it
as a Bevy message. Admission retains it while a layout barrier is pending. For
callers needing an operation receipt, use `submit_host_request` and
`take_host_result`. Requests retain their session, source and target area;
stale layout actions and effects from failed panels are rejected. Save receipts
identify the exact committed revision. The internal admitted-request queue and
receipt table remain durable rather than depending on message retention.
The message envelope lives in `bed-api`, so feature plugins can emit it
without depending on the desktop composition crate. `HostEffects` remains a
compatibility surface for existing rich frontends and the synchronous effect
outbox for registered commands, panel actions and draft commits.

Custom typed contents implement `DocumentModel`, whose change returns its
inverse and whose encoding supplies bytes for saving. `create_model_document`
registers the contents; panels use `DocumentBinding` and read through `model`.
`edit_model` validates the expected revision and applies the entire change to a
clone before committing contents and history. `undo_document`, `redo_document`,
`save_document`, `document_revision` and `document_dirty` work for both default
and typed documents. Hosts with custom storage acknowledge the exact persisted
revision through `mark_model_saved`. A plugin can also keep its own model without
joining this document lifecycle.
Model mutation and persistence stop when the shared session shuts down.

The default `DocumentClosePolicy` rejects closing the last view of a dirty
document or quitting with unsaved changes. Applications can select Save or
Discard, or install their own `WorkbenchClosePolicy` before core. A failed save
retains the panel and document. Core does not select workspace storage or display
application-specific save prompts.
Close approval never retires a frontend. `closing_panels(world)` exposes the
complete set during an aggregate layout/restore preflight, so other closing
views cannot be mistaken for surviving views. Retirement hooks run only at the
workbench's actual close commit.

## Layout and feature persistence

Core supplies one active workbench layout. Add
[`LayoutsPlugin`](../bed-plugin-layouts/README.md) for an application-selected collection
of numbered or named layouts. The layout crate retains suspended panels and
returns serializable snapshots; the host supplies its storage.

`FeatureStateContribution` registers save/restore systems for a feature's state,
and `FeatureStateChanged` reports changes through Bevy messages. Neither includes
a workspace or filesystem location. Applications invoke these systems at their
own persistence boundary and preserve unknown contributions as needed. See the
[bEd adapter](../bed-app/README.md) for workspace policy.

## Plugin icon sources

A plugin declares its own local filenames and registers them in its namespace.
For example, `example.graph` can require `node.svg`; its resolved icon key is
`example.graph.node`. Another plugin may also use a local `node.svg` without
sharing artwork or configuration.

```rust,no_run
use bed_core::{api, bevy::prelude::*, bed_ui::icons::{IconDefinition, IconDirectories, Icons}};

struct GraphPlugin;
impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        let directory = app.world().non_send::<IconDirectories>()
            .get("example.graph").map(std::path::Path::to_path_buf);
        app.world_mut().non_send_mut::<Icons>().register(
            "example.graph",
            &[IconDefinition {
                name: "node",
                filename: "node.svg",
                embedded_svg: Some(include_bytes!("assets/node.svg")),
            }],
            directory.as_deref(),
        ).expect("valid graph icon source");
        // Register the panel/command, then reference "example.graph.node"
        // with registrar.panel_icon(...) or registrar.command_icon(...).
    }
}
```

Applications insert the non-send `IconDirectories` before installing core and
feature plugins. Each map entry selects a directory for one plugin:

```rust,no_run
use bed_core::{BedCorePlugin, bevy::prelude::*, bed_ui::icons::IconDirectories};

let mut directories = IconDirectories::default();
directories.0.insert("bed.terminal".into(), "my-icons/terminal".into());
directories.0.insert("bed.core".into(), "my-icons/chrome".into());
let mut app = App::new();
app.insert_non_send(directories).add_plugins(BedCorePlugin::default());
```

Directory files override that plugin's embedded defaults. Missing files retain
their embedded default; a required file without a default, malformed SVG or
invalid/duplicate contract returns a registration error before any of that
contribution is published. Third-party SVG sets can be packaged as embedded
bytes or supplied in a directory using the plugin's filenames. SVG silhouettes
are rasterized to monochrome images and tinted using the current theme.

Core supplies embedded `bed.core.*` artwork without discovering bEd assets or
configuration. Hosts may supply source images before core installation; those
images are preserved when shared defaults are filled. The desktop owns native
and GPU realization, refreshes changed source images, and recreates GPU textures
after device replacement. Exact namespaced keys work in both ImGui and native
toolbars. Unqualified shared names remain supported while existing callers
migrate.

## Terminal launch integration

`TerminalPlugin` supplies ordinary shell panels, process/input handling, fonts
and runtime appearance. Set `TerminalWorkspace` to choose the directory for new
shells. Its errors are observable through `bed_terminal::TerminalActivity`;
ordinary hosts need no executable helper dispatch or application request queue.

`TerminalAppIntegrationPlugin` opts into application workspace projection, panel lifecycle
and clipboard integration, shell commands such as `+open`/`+workspace`, and
terminal file links. The application host resolves those requests. Install it after
`TerminalPlugin` and dispatch `bed_plugin_terminal::run_helper` before constructing the
GUI, because private helper invocations execute the host binary:

```rust,no_run
fn main() {
    if let Some(result) = bed_plugin_terminal::run_helper(
        std::env::args_os(),
        std::env::var_os("BEDTERM_SOCKET").is_some(),
    ) {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    // Build the application and explicitly add TerminalAppIntegrationPlugin.
}
```

Debugger launch preflight likewise belongs before GUI startup when an
application installs the debugger integration. bEd keeps its command-line
parsing, configuration and smoke commands in its own binary.
