# bedtime-terminal2

Local PTY terminal panels for UI2 and Workbench2. The application installs one
plugin and composes concrete panel instances. Creation menus are host policy;
Workbench has no built-in type picker.

```rust,ignore
use bedtime_terminal2::{TerminalPanel, TerminalPlugin};
use bedtime_workbench2::InArea;

app.add_plugins(TerminalPlugin); // After the application's Bevy/Workbench/UI setup.
commands.spawn((TerminalPanel::default(), InArea(area)));
```

The descriptive key is `bedtime.terminal`, with label **Terminal**; it is not a
construction-registry entry. The component requires `PanelInstance::of::<Self>()`,
so metadata and drawing exist as soon as the spawn applies. The plugin creates no
workspace, window, camera or renderer. No panel registration is needed.

Instance tab labels follow the local foreground process, falling back to the
launched shell name and then **Terminal** before a runtime exists. The shared PTY
worker refreshes the process observation every 400 ms on macOS/Linux, including
quiet jobs and hidden panels; updating the tab does no OS queries on the UI thread.
Labels return to the shell when a foreground job finishes. OSC window titles stay
in `snapshot().title` and do not override process labels. Process names are
best-effort OS observations (interpreter launchers can be named after their script),
not full command lines or remote-process inspection. Labels omit control characters
and are limited to 64 characters. Fresh panels use fallback labels until their
runtime supplies a process observation.

Explicit launch options can be attached in the same spawn:

```rust,ignore
use bedtime_terminal2::{TerminalOptions, TerminalPanel, TerminalShell};

let options = TerminalOptions {
    working_directory: Some(project_directory),
    // Omit shell to use the user's normal interactive login shell.
    shell: Some(TerminalShell::new("/bin/sh", vec!["-i".into()])),
    ..Default::default()
};
commands.spawn((
    TerminalPanel::new(options),
    InArea(area),
));
```

Required components preserve supplied state; there is no panel initialization
pass. Spawning selects the destination tab without changing area focus. Changing
`options` affects only the next
start or explicit `restart()`. Cwd/environment setup affects the child, not the
application. Without a directory override, the child inherits the application's
working directory. `TERM_PROGRAM` defaults to `bed2` unless explicitly overridden.

## Local shell commands

Applications opt in by registering aliases and handling helper invocations before
constructing the App:

```rust,ignore
use bedtime_terminal2::ShellCommandsAppExt;

fn main() -> bevy::app::AppExit {
    if let Some(exit) = bedtime_terminal2::handle_shell_command() {
        return exit;
    }
    let mut app = bevy::prelude::App::new();
    // Install normal Bevy, UI2, Workbench and TerminalPlugin capabilities.
    app.add_shell_command(["+open", "+o"], open_from_shell)
        .expect("Unique application shell aliases");
    app.run()
}
```

`run_shell_command(args_os)` is the lower-level, injectable variant, returning
`Option<io::Result<()>>`. Both recognize the executable basename, so ordinary
application arguments remain application-owned. A recognized helper failure must
exit instead of starting a GUI. `handle_shell_command()` prints the error and
returns an unsuccessful `AppExit`; neither helper terminates the process itself.

`add_shell_command` accepts a `fn(&mut World, ShellInvocation)` handler. Register
aliases before the first update; malformed/duplicate registrations fail atomically.
No aliases means no command socket or helpers. Startup creates a private Unix
socket and executable symlinks to this application's binary. Each child launch
receives its helper PATH, endpoint and a fresh runtime token; saved launch options
and the application's environment are untouched. Inherited parent endpoints and
origins are removed, including when commands are disabled in a nested application.

Handlers run during `TerminalSystems::Update`, after terminal observations. The
invocation contains native `OsString` arguments, the helper's current directory,
and the validated originating panel/workspace, independent of current focus or
workspace activation. Restarted, stopped or closed origins are rejected. Command
semantics, file/viewer routing and quit/readiness policy belong to the application.
Workbench's argument-free contextual command API is unchanged.

Complete `invocation.reply` with `Result<(), String>`. For asynchronous operations,
retain it with the pending operation and complete it after its actual result;
dropping it without completion returns an explicit abandonment error. Socket
polling never waits for the handler's work. The helper waits at most 60 seconds;
a timeout does not roll back an accepted application operation and is not retried.
`shell_socket_path(world)` exposes the live endpoint
for read-only integration. Socket/helper cleanup follows application exit/drop;
no system-wide helper installation or separate executable build is needed.

## Ownership and lifecycle

- An unshown panel is `Pending`. Its first content-size observation allows the
  ordinary Update system to admit a panel-owned `WorkspaceWork` task.
- That single worker creates and owns the PTY, child, parser and terminal model.
  No second supervisor thread or application-global session registry is installed.
  A bounded ordered input handoff retries backend backpressure, including key/
  mouse release and focus cleanup. New process input pauses while it is backed up;
  exceeding its 512-entry/4 MiB limit or permanently rejecting input explicitly
  stops the runtime rather than leaving held input or executing a partial line.
- The worker reads and parses output in hidden panels and inactive workspaces,
  independently of `background_updates`. Only drawn views request fresh viewport
  snapshots and prepare text. A hidden panel's inspected viewport may be stale
  until it is shown again; this does not pause terminal I/O.
- Moving tabs within a workspace retains the same session. Work is owned by the
  workspace in which it was admitted, with the panel as its shorter-lived owner.
- The application-facing `stop()` terminates the child and keeps the transcript;
  there is no Stop button in the canvas. Shell exit exposes its status and also
  retains scrollback, selection and copying. **Restart** finishes
  the old runtime before admitting a fresh one, with a fresh transcript. Restart
  requires positively confirmed child cleanup, not merely a cancelled task result.
- Closing/despawning a panel requests cleanup without waiting on the UI thread.
  Workspace closure and `Workspaces::shutdown_work()` track actual completion.
  The application's existing save/discard/quit policy remains authoritative; it
  must keep updating until `work_finished()` before exiting.
- Session persistence and restore recipes are application-owned; core Workbench
  snapshots/restoration have been removed. Live processes, transcripts and runtime
  handles are not persistent state. Reconstructed concrete panels start fresh and
  lazily when shown.

`status()`, `process_id()`, `snapshot()` and `error()` expose observations for
application UI/inspection. A PID is not a persistent identity or a signaling API.
Shell exit status is distinct from runtime status: the emulator can remain managed
and usable after its shell has exited.

## Implementation boundary

The v2 crate reuses `bedtime-terminal` with default features disabled for its
headless `rio-vt` model, bounded command queues, keyboard/paste encoding and Unix
PTY implementation, plus its bounded local shell-command transport. It does
**not** install v1's ImGui frontend, `BedTerminal` or terminal plugin. V2 owns its
command registrations and terminal-origin validation. The shared backend's prepared
worker runs directly in the workbench-owned task.

The frontend is an ordinary egui fixed-grid canvas. Workbench supplies panel
layout/clipping; the terminal owns focus-sensitive input and scrolling. Registered
application chords remain reserved for Workbench's post-draw shortcut resolution.

## Appearance

A running shell has no duplicate title or toolbar beneath the workbench tab.
The canvas has a 14-point horizontal / 12-point vertical inset, with its scroll
indicator tucked into the right inset and aligned to the content's top/bottom.
Cell/selection/cursor clipping preserves that padding even while an older, larger
viewport snapshot is awaiting resize. Startup, exit, failure and input-backpressure
notices appear only when needed; exited/failed sessions still offer **Restart**.
Shell titles remain available in the snapshot without adding another heading.

The canvas follows the surface's active UI2 `Theme`: panel background, default
text, selection, scrollbar and font size. Its 16 normal/bright ANSI colors come
from `theme.palette.terminal`, with bed1's Everforest Medium palettes in the
built-in dark/light themes. Host palette edits recolor existing indexed output
without restarting the shell or clearing its transcript. The shared core repairs
contrast against the terminal background, just as in bed1; swatches can therefore
differ from their resolved terminal colors. Explicit xterm-256/truecolor output
and shell OSC palette/default/cursor overrides remain intact. OSC resets return
to the current host theme, not the startup theme.

UI2 supplies Bedfont (Nordwand Mono) regular and real bold with matching cell metrics and egui
Unicode fallbacks. Faint takes precedence over bold weight and bold-to-bright
color. Italics use egui's synthetic shear. Grid origins, cells and glyph placement
align to physical pixels at the current display scale; wide/combining clusters
remain in their fixed spans. Cursor/text blink and selection-color changes repaint
without rebuilding unchanged text layouts. Hosts replacing egui's font definitions
must retain UI2's `fonts::monospace_bold` family with a matching bold face; start
from `bedtime_ui2::fonts::definitions()` to preserve the bundled defaults.

## Input

Copy/paste/select-all use Cmd+C/V/A on macOS and Ctrl+Shift+C/V/A elsewhere.
Ctrl+C and Ctrl+V remain ordinary terminal control input on non-macOS. Shift
bypasses application mouse reporting for local selection and scrolling;
Shift+PageUp/PageDown/Home/End navigate scrollback. Double/triple click select
words/lines, and Alt-drag selects a rectangle. Focus loss, hidden panels and modal
UI release this canvas's held input without consuming another widget's events.

## Initial scope

- Native backend: macOS/Linux (Unix PTY); Windows PTY support is not implemented.
- Local processes only: no SSH session or remote-workspace integration.
- Local `+` helpers are opt-in application commands; no shell acceptance hooks
  or app-command composer. Ordinary input remains terminal input.
- Inline terminal images are disabled, including graphics protocol processing.
  The bounded streaming filter also omits DCS protocols (including DCS queries and
  passthrough) and OSC 1337 user variables. CSI synchronized updates and ordinary
  ANSI/CSI, titles, colors, cwd reports and OSC 8 hyperlinks remain supported by
  the shared core. Links are retained in the model, not activated by this canvas.
- egui shapes each complete terminal cluster in its fixed 1/2-column span. Glyph
  coverage depends on the host fonts; cross-cell contextual shaping, bidi
  reordering and color emoji are not implemented.
- egui does not preserve numpad identity, keyboard lock state or an unmodified
  Unicode keysym; the mapper preserves available native text/physical base keys,
  modifiers, repeats, releases and IME commits without inventing missing metadata.
- Default terminal backgrounds inherit Workbench's single panel-background fill,
  including its opacity. Explicit ANSI/truecolor backgrounds, reverse-cell fills
  and shell OSC background overrides remain opaque. Terminal-specific appearance/
  settings are not yet persisted.
- No debugger session sharing, detached sessions, terminal-specific nested tabs or
  live process restoration.

Native terminal input/rendering and process-tree cleanup need platform-specific
verification. Unix shutdown handles the shell and its foreground process group;
this is not a sandbox or a guarantee against deliberately detached processes.

## Verification

```sh
cargo test --offline --locked -p bedtime-terminal --no-default-features --lib
cargo test --offline --locked -p bedtime-terminal2
cargo check --offline --locked -p bed2
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_terminal.py
python3 bedtime2/tools/smoke_open.py
```

The test suite targets concrete composition/lazy startup, actual local PTY I/O,
resize, exit/transcript selection, restart, hidden/inactive operation, failed
startup, panel despawn, workspace closure and shutdown admission. The native smoke
uses application-owned panel creation and injected input rather than a test-only terminal
write endpoint. Its appearance scenario covers dark/light/custom ANSI palettes,
normal/bright colors, live font-size changes, retained sessions/transcripts,
truecolor and OSC override/reset behavior. It also checks that the running canvas
has no duplicate heading or Stop button, while the panel tab and exit/restart UI
remain available. These commands are verification recipes, not a claim that they
have passed on every platform.

Shell commands were verified on 2026-10-22 on macOS: all 47 Terminal2 tests and
11 shared transport tests passed. The new tests exercise deferred/error replies,
native argument/cwd bytes, inactive-workspace origins, restart/close rejection,
abandoned handlers and atomic registration. Unix socket tests require execution
outside the macOS filesystem sandbox. `smoke_open.py` passed against the native
automation application, including pre-GUI helper failure and saved-session loading
through the shell's cwd. See the [application opening contract](../../../bed2/README.md#opening-from-the-terminal).

The results below are historical, predating the entity-first refactor; they do
not establish verification of the current in-progress worktree.

Verified on 2026-10-18 on macOS/Metal: all 55 headless core tests, 84 default-feature
v1 terminal tests, and 36 Terminal2 tests passed; the UI2/Workbench2/bed2 suites
also passed (23/166/10 tests, plus the workbench document doctest). Normal bed2
compilation and the v1 terminal plugin's optional remote build passed. The native
terminal scenario passed with 10 nontransparent, content-change-checked captures
in `target/terminal-smoke-validated`, using a pinned copy of the automation binary.
The existing workbench and task/process native scenarios passed too. Linux was
not run; Windows remains unsupported. These checks do not establish a performance
benchmark or complete shell/font/protocol compatibility.

The appearance update was verified on 2026-10-18 on macOS/Metal: all 39 Terminal2,
27 UI2, 166 Workbench2, 11 automation and 10 bed2 unit tests passed, along with the
workbench document doctest and a normal bed2 compilation check. The expanded
native terminal scenario passed with 14 nontransparent, content-change-checked
captures in `target/terminal-rendering-smoke`; the existing workbench scenario
passed with nine captures in `target/terminal-rendering-workbench-smoke`. Both
used the pinned `target/bed2-terminal-rendering-smoke` executable. Native Linux
and non-macOS display/input behavior remain unverified.

The header/padding polish was subsequently verified on the same platform with all
40 Terminal2 tests, normal bed2 compilation, and the native terminal scenario's
14 checked captures in `target/terminal-polish-smoke`, using the pinned
`target/bed2-terminal-polish-smoke` executable.

Process tab labels were verified on 2026-10-18 on macOS/Metal with all 44 Terminal2
tests and normal bed2 compilation. The native scenario passed with 15 checked
captures in `target/terminal-process-tabs-smoke`, using the pinned
`target/bed2-terminal-process-tabs-smoke` executable; it checks a quiet foreground
`sleep` job, the rendered tab name, return to the shell, and OSC-title independence.
The suite also covers default-background compositing at multiple alpha values;
the native scenario does not yet assert per-pixel panel alpha. Linux process names
have not been verified in a native run.
