# bed-plugin-terminal

`TerminalPlugin` contributes ordinary shell panels to the workbench. It owns
process/input handling, fonts and runtime appearance. `TerminalLaunchContext`
chooses the directory for new shells; an empty directory inherits the host's
working directory. Errors are observable through
`bed_terminal::TerminalActivity`.

The default build supports local shells. Enable the `remote` feature for SSH
sessions and remote application workspace projection; it forwards the `bed-terminal`
transport and `bed-documents` service integration. A remote workspace without
that feature reports an unavailable panel instead of starting a local shell.

Register `bed_plugin_terminal::launch_registration()` with
[`bed_app::Application`](../bed-app/README.md). Its deferred installer adds
`TerminalPlugin` and `TerminalAppIntegrationPlugin` after the foundation, and
its early `+` executable handler forwards shell invocations before GUI startup.

Open `bed.terminal.panel` through workbench requests or use the contributed
`bed.terminal.new` command. The icon contract is `terminal.svg`, with embedded
default and exact key `bed.terminal.terminal`; applications can override its
owner directory through launcher `IconDirectories`.

`TerminalAppIntegrationPlugin` projects shared file context into new-shell
launch context and adds panel lifecycle, clipboard and file-link integration.
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
