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

Install [`BedCorePlugin`](../bed-core/README.md) first, then `TerminalPlugin`.
Open `bed.terminal.panel` through workbench requests or use the contributed
`bed.terminal.new` command. The plugin's icon contract is `terminal.svg`, with an
embedded default and exact key `bed.terminal.terminal`. Applications can override
it through the `bed.terminal` entry in
[`IconDirectories`](../bed-ui/README.md#icon-contracts).

## Application integration

`TerminalAppIntegrationPlugin` projects the shared `FileContext` into the launch
context and installs panel lifecycle, clipboard and terminal file-link integration. The application host resolves those requests. Add it explicitly
alongside `TerminalPlugin`; it can also install `TerminalPlugin` when absent.

Applications exposing registered shell commands dispatch `run_helper` before constructing the
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

`TerminalPlugin` owns a local command socket. It creates executable helpers only
for aliases contributed through `WorkbenchRegistrar::shell_alias`; command owners
receive raw argument bytes, calling directory and pane via `ShellInvocation`.
The document owner contributes `+open`/`+o`, workspaces contribute `+workspace`/`+w`,
and mascots contribute `+ducky`. Third-party aliases use the same transport.
Missing/disabled commands return errors; acknowledgement waits for the owning
operation's completion. Historic helper wire messages remain accepted.

Hosts without shell registrations need no helper preflight. The application
continues to own command-line parsing, configuration and persistence. Remote
terminal file links remain supported; shell-command transport is local only.
