# bed-ui

Shared presentation types, ImGui helpers and plugin-owned icon sources. The
desktop handles native images and GPU textures; plugins register source artwork.

## Icon contracts

Each plugin declares local filenames and registers them under its own namespace.
For example, `example.graph` can declare `node.svg` and reference the resulting
key `example.graph.node`. Another plugin can declare its own `node.svg` without
sharing artwork or configuration.

```rust,ignore
use bed_ui::icons::{IconDefinition, IconDirectories, Icons};

let directory = world.non_send::<IconDirectories>()
    .get("example.graph").map(std::path::Path::to_path_buf);
world.non_send_mut::<Icons>().register(
    "example.graph",
    &[IconDefinition {
        name: "node",
        filename: "node.svg",
        embedded_svg: Some(include_bytes!("assets/node.svg")),
    }],
    directory.as_deref(),
)?;
```

The owned application launcher accepts `IconDirectories` before installing the
foundation and feature plugins. Each entry chooses a directory for one plugin:

```rust,ignore
let mut directories = IconDirectories::default();
directories.0.insert("bed.terminal".into(), "my-icons/terminal".into());
directories.0.insert("bed.core".into(), "my-icons/chrome".into());
let options = bed_launch::DesktopOptions {
    icon_directories: directories,
    ..Default::default()
};
launch.run(options, |app| {
    app.add_plugins(MyPanelPlugin);
    Ok(())
})?;
```

A directory file overrides that plugin's embedded default. A missing file keeps
its embedded default. Missing required artwork, malformed SVG, invalid names or
duplicate contracts return an error before any of the registration is published.
Third-party SVG sets may be embedded or provided in directories using the
plugin's filenames. SVG silhouettes are converted to monochrome images and
tinted using the current theme.

The built-in contracts are:

| Namespace | Local filenames |
| --- | --- |
| `bed.core` | Keys in the [shared catalog](assets/icons/catalog.tsv), each with `.svg` |
| `bed.terminal` | `terminal.svg` |
| `bed.http` | `request.svg` |
| `bed.explorer` | `files.svg`, `folder.svg`, `folder-open.svg`, `chevron-right.svg`, `chevron-down.svg` |
| `bed.git` | `branch.svg`, `refresh.svg`, `fetch.svg`, `pull.svg`, `push.svg`, `stage.svg`, `unstage.svg` |
| `bed.debug` | `debug.svg` |
| `bed.editor` | `diagnostics.svg`, `language-server.svg`, `document.svg`, `hex.svg`, `navigation.svg` |

`Icons::bundled()` supplies shared `bed.core.*` artwork without an application
asset directory. `register_core_defaults` fills missing shared images while
preserving images already supplied by a host. A plugin can reuse bundled artwork
through `Icons::bundled_svg` while retaining its own namespace and override
directory. Panel and command contributions reference these exact keys through
`panel_icon` and `command_icon` in the workbench API.

`Icons::images` owns source images. The desktop refreshes native and GPU images
when those sources change and recreates GPU textures after device replacement.
Namespaced keys resolve exactly in both ImGui and native toolbars. All lookups use exact namespaced keys such as `bed.core.close`.

## Shared controls and live themes

The application selects its initial `UiTheme` with `DesktopOptions::theme`, then
can publish live changes through the desktop's `DesktopTheme` resource.
Its palette, font options, control metrics and motion policy are prepared before
opening the frame. Theme selection and disk formats belong to the application.
`UiRuntime` owns the context-local lifecycle and publishes immutable
`UiFrameContext` snapshots. Components do not read settings or perform actions.

```rust,ignore
let context = bed_ui::UiFrameContext::for_ui(ui);
if bed_ui::components::button(ui, &context, "Save") {
    // Return the user's intention to the panel's owner.
}
if ui.is_item_hovered() {
    bed_ui::components::tooltip_text(ui, &context, "Save this document");
}
```

Controls, compact controls, icon buttons, tooltips, dialog scopes and context-menu
scopes share the same prepared metrics. Scoped styles restore their caller's
style on drop. Popup metrics remain consistent inside square panels. A complete
frontend drawing scope includes third-party and footer popups in opening motion.
Direct ImGui fixtures without a runtime inherit native colors and the common
default metrics; `UiRuntime` supplies customized themes without a workbench.

`DesktopOptions::theme` also establishes font, opacity and motion preference
defaults. User preferences can override those defaults; applications resolve
their selected theme before publishing the next frame's `DesktopTheme`.
