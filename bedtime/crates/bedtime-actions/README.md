# bedtime-actions

Actions (named things a user can do), the handlers that perform them, and
their keys, for Bevy apps. Needs no other bedtime crate; `settings` makes keys
settings ([`bedtime-settings`](../bedtime-settings/README.md)), and `ui`
collects shortcuts from egui.

```rust,ignore
app.add_plugins(ActionsPlugin);
```

## Actions and handlers

An action (a command the user can run: from a menu, a key, a button) is a
marker type implementing `Action`, with a stable dotted `KEY`, a `LABEL`, and optional
`DEFAULT_KEYS` and `REPEATS` (whether held keys run it again).

```rust,ignore
struct Save;
impl Action for Save {
    const KEY: &'static str = "document.save";
    const LABEL: &'static str = "Save";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyS)];
}

app.register_action::<Save>(); // known, bindable, listed
// Also sets its global handler, available only when there's something to save.
app.add_action_handler::<Save>(Handler::new(save_focused).available_when(can_save));
// An entity's own handler (a panel's, say).
commands.spawn((MyEditor, ActionHandler::<Save>::new(Handler::new(save_editor))));
```

There is one handler type, `Handler`: what the action does
(`fn(&mut World, ActionInvocation) -> ActionResult`) and, optionally, whether it
can run now (`fn(&World, Option<Entity>) -> bool`, given the entity). An action
has at most one global handler, kept in `ActionRegistry`
(`set_global_handler::<A>`), and any entity may carry an `ActionHandler<A>`
component. When an action runs for an entity, that entity's handler wins;
otherwise the global handler runs, and is told the entity too. An entity's
handler that isn't available makes the action unavailable for it rather than
falling back. The invocation also names its `source` (e.g. the menu item), so
a handler can read details from it.

**Focus.** Actions run for the focused entity unless a request says otherwise.
What's focused is the app's: `ActionFocus` holds a function that says
(bedtime-workbench makes it the focused panel). Without one, only global
handlers run.

## Keybindings

`Keybindings` is a plain table from action to chords, filled with each
action's defaults on registration. With the `settings` feature, each
action's chords are also the setting `keys.<KEY>` (`Keybinding<C>`), so they
live in settings layers like any other setting: a higher layer's list replaces
lower ones for that action, and an empty list unbinds it. Chords are written
portably, e.g. `"Primary+Shift+Z"` (Primary is Cmd on macOS, Ctrl elsewhere).
`world.actions().keys_in` / `keys_at` / `keys_overridden_above` and
`world.actions_mut().set_keys_in` read and write one layer's chords by action
id, for settings UIs.

Actions may share a chord. Pressing it runs the bound action handled in the
most specific scope: the focused entity, then global. If the same scope handles
several of them, nothing runs and `BindingConflict` is sent. Modifiers match
exactly.

## Running actions

`ActionRequests` queues actions (`invoke::<Save>()`, or a request with
`InvocationContext::{Focused, Global, Entity(entity)}`). Dispatch runs them in
order in `PreUpdate` and sends `ActionFinished` with each result, and whether
a handler ran. Requests made by handlers run next frame.
`world.actions().available` and `shortcut_for` answer the same questions for
menus.

## The `ui` feature

Shortcuts are collected from the focused window's egui context after widgets
have taken the keys they want: a focused text field keeps typing keys, and
modals and popups block shortcuts. egui quits on Cmd+Q by default; turn off its
`quit_shortcuts` to bind that key. With `settings` too,
`ui::keybindings_section` is the Keyboard Shortcuts section of a settings panel:
every action and its keys in a layer, re-recorded by clicking and pressing new
ones.

```sh
cargo test -p bedtime-actions --all-features
```
