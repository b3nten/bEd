# bEd 2

Run the workspace demo:

~~~sh
cargo run --offline -p bed2
~~~

Playground and Scratch each have a separate Bevy SubApp. Both start with editable
notes, a background-work panel and a 3D viewport. The Reading layout has separate
notes. Switch workspaces and layouts to inspect retained state.

The host toolbar opens and closes workspaces and controls whether each continues
updating while inactive. Exactly one workspace stays active; closing it selects
a live replacement. Closing the final workspace is disabled.

In Background work, run a cancellable two-second task or start an interactive
pipe process. Send text and switch workspaces; output stays with its owner.
Closing the panel cancels its work. Closing a workspace cancels all its work and
retires its world after cleanup. The process example uses pipes; PTY terminal
emulation is not implemented here.

Panels retain Blender-style tab, split, join and resize interactions.
Right-click an empty area to open Notes, Background work, Widget gallery or Theme editor.

Scene angles live in each workspace. The application projects active state into
shared host render entities; Workbench projects viewport images and geometry
for UI texture registration and resizing. Workspace render entities are not
automatically extracted.

Widget gallery and Theme editor open docked tabs in the active workspace and
layout. Clicking again focuses the existing tab. Their panel state belongs to
the workspace; changes to the window theme, motion, icons and clipboard use
shared host services. To run the gallery by itself:

~~~sh
cargo run -p bedtime-ui2 --example gallery
~~~

The independent Bevy game example remains available:

~~~sh
cargo run -p bed2-demo-game --example standalone
~~~

For rendered verification, including screenshot content, state isolation,
background suspension and process cleanup:

~~~sh
cargo run --offline -p bed2 -- --workspace-smoke-dir target/workspace-smoke
~~~

The previous --workbench-smoke-dir and --viewport-smoke-dir names select this
workspace smoke test. It uses an in-memory clipboard and captures Playground,
Scratch, the remaining workspace after closing Playground, and both docked tools.

--egui-menu selects the in-application menu. --ui-first reverses UI/workbench
plugin registration. --compose-workbench installs individual plugins and omits
native menus. --blur enables native material.

See the [workspace API](../bedtime2/crates/bedtime-workbench2/README.md) for scoped
world access, plugin setup, execution order and shutdown responsibilities.
