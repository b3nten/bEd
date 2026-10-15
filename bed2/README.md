# bEd 2

Application host for the second Bedtime foundation:

```sh
cargo run -p bed2
```

The application configures Bevy's window and renderer and installs
`DesktopPlugin`, `UiPlugin` and `WorkbenchPlugin`. Its widget gallery demonstrates
Everforest Medium light/dark themes, reusable controls, motion and clipped lists.
Use **Browse icons** to explore the SVG catalog and switch replacement packs at runtime.
Theme entities and shared view state live in ECS; controls own UI-local animations.

The **3D viewport** shows a spinning Bevy cube with a game-owned fullscreen shader.
Resize the panel to resize its render target. **Full-window scene** retargets the
same camera to the window and draws floating UI and a HUD above it; **Panel**
returns to texture embedding. Close the panel to use the gallery, then use
**Open 3D viewport** to reopen it.

The scene is a separate Bevy-only crate. Run it without Bedtime:

```sh
cargo run -p bed2-demo-game --example standalone
```

For native renderer verification:

```sh
cargo run -p bed2 -- --viewport-smoke-dir target/ui2-viewport-smoke
```

Copy Greeting and Toggle Copy Availability buttons reference the same typed
command entities as the macOS native menu. Availability changes affect both
presentations. Quit requests and window-close requests reach application-owned
close handling. Escape requests quit when UI capture allows it; dialogs and text
fields retain their input.
The demonstration Copy Greeting shortcut uses Cmd/Ctrl+Shift+C so ordinary
Cmd/Ctrl+C remains available to text fields.

The application orders UI drawing before workbench ingress and dispatch in
`PostUpdate`. Commands therefore execute after the borrowed ImGui frame ends.

For a gallery without workbench:

```sh
cargo run -p bedtime-ui2 --example gallery
```

See [migration step 2](../MIGRATION/step-2.md) and the
[UI crate documentation](../bedtime2/crates/bedtime-ui2/README.md).
