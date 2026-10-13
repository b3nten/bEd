# bed-plugin-terminal

`TerminalPlugin` contributes ordinary shell panels to the workbench. It owns
process/input handling, fonts and runtime appearance. `TerminalWorkspace`
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

`TerminalAppIntegrationPlugin` opts into application workspace projection, panel lifecycle
and clipboard integration, shell commands such as `+open`/`+workspace`, and
terminal file links. The application host resolves those requests. Add it explicitly
alongside `TerminalPlugin`; it can also install `TerminalPlugin` when absent.

Applications using this adapter dispatch `run_helper` before constructing the
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

Ordinary shell panels need no helper preflight or application request handling. The
application continues to own command-line parsing, configuration and persistence.
