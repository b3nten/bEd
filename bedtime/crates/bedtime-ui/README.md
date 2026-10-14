# bedtime-ui

Shared presentation types, ImGui helpers, and a shared icon vocabulary.
Applications select artwork; plugins consume keys. The desktop owns native
images, GPU textures, and their lifetimes.

## Icons and runtime packs

`Icons::default()` provides embedded monochrome artwork for every key in the
[shared catalog](assets/icons/catalog.tsv). Plugins reference keys such as
`folder`, `terminal`, and `git-branch` through `get`/`find`, `panel_icon`, or
`command_icon`. Exact lookups return `None` for missing textures; `get_for_file`
selects a filename/extension association and falls back to `default`.

Applications supply partial or complete SVG overrides:

```rust,ignore
use bedtime_ui::icons::{IconPack, Icons};

let pack = IconPack::from_directory(std::path::Path::new("my-icons"))?;
let options = bedtime::DesktopOptions {
    icon_pack: pack,
    ..Default::default()
};
launch.run(options, |app| {
    app.add_plugins(MyPanelPlugin);
    Ok(())
})?;
```

Directory filenames are shared keys with `.svg`, for example `folder.svg`.
Non-SVG files are ignored, allowing attribution files in the pack. Applications
can also construct `IconPack` directly from a map of keys to SVG bytes.

To switch packs at runtime, replace the pack before desktop preparation:

```rust,ignore
world.non_send_mut::<Icons>().replace_pack(&next_pack)?;
// An empty pack restores all embedded defaults.
world.non_send_mut::<Icons>().replace_pack(&IconPack::default())?;
```

Replacement validates and rasterizes the whole pack before publishing anything.
Unknown keys or malformed SVGs preserve the current artwork and texture IDs.
Every replacement resolves against embedded defaults, never the previous pack.
Omitted keys therefore return to defaults when switching packs.

Source images are read-only through `Icons::images()`. The desktop reconciles
changed native and GPU images before drawing and recreates GPU textures after
device replacement. Plugins look up texture IDs each frame rather than retaining
them. Artwork defines monochrome silhouettes; draw sites tint them with current
UI colors. Color themes and icon packs remain independent.

## Shared controls and live themes

The application selects its initial `UiTheme` with `DesktopOptions::theme`, then
can publish live changes through the desktop's `DesktopTheme` resource.
Its palette, font options, control metrics and motion policy are prepared before
opening the frame. Theme selection and disk formats belong to the application.
`UiRuntime` owns the context-local lifecycle and publishes immutable
`UiFrameContext` snapshots. Components do not read settings or perform actions.

```rust,ignore
let context = bedtime_ui::UiFrameContext::for_ui(ui);
if bedtime_ui::components::button(ui, &context, "Save") {
    // Return the user's intention to the panel's owner.
}
if ui.is_item_hovered() {
    bedtime_ui::components::tooltip_text(ui, &context, "Save this document");
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
