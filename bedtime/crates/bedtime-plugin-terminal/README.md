# bedtime-plugin-terminal

`TerminalPlugin` contributes ordinary shell panels to the workbench. It owns
process/input handling, fonts and runtime appearance. Shared UI appearance supplies
foreground/background colors; an optional `api::presentation::TerminalPalette`
supplies ANSI colors. `TerminalLaunchContext`
chooses the directory for new shells; an empty directory inherits the host's
working directory. Errors are observable through
`bedtime_terminal::TerminalActivity`.

The default build supports local shells. Enable the `remote` feature for SSH
sessions and remote application workspace projection; it forwards the `bedtime-terminal`
transport and `bedtime-documents` service integration. A remote workspace without
that feature reports an unavailable panel instead of starting a local shell.

Register `bedtime_plugin_terminal::launchers()` with
[`bedtime::LaunchBuilder`](../bedtime/README.md). Its early `+` executable
handler forwards shell invocations before GUI startup. Install `TerminalPlugin`
in the desktop callback; it installs its directory, lifecycle and clipboard integration.

Open `bedtime.terminal.panel` through workbench requests or use the contributed
`bedtime.terminal.new` command. The icon contract is `terminal.svg`, with embedded
default and exact key `bedtime.terminal.terminal`; applications can override its
owner directory through launcher `IconDirectories`.

`TerminalPlugin` projects shared file context into new-shell launch context and adds
panel lifecycle, clipboard and file-link integration.
Input policy projects the shared resolved keymap before native interception,
including global/panel chords and common clipboard overrides. A remote file
context without transport support reports an unavailable panel.

`TerminalPlugin` owns a local command socket. It creates executable helpers only
for aliases contributed through `WorkbenchRegistrar::shell_alias`; command owners
receive raw argument bytes, calling directory and pane via `ShellInvocation`.
The document owner contributes `+open`/`+o`, workspaces contribute `+workspace`/`+w`,
and mascots contribute `+ducky`. Third-party aliases use the same transport.
Missing/disabled commands return errors; acknowledgement waits for the owning
operation's completion. The read-only `shell_socket_path(world)` exposes the active private endpoint for integrations.

Applications continue to own command-line parsing, configuration and persistence. Remote
terminal file links remain supported; shell-command transport is local only.
