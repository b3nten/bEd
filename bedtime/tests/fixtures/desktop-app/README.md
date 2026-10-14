# Independent desktop consumer

This Cargo workspace deliberately does not inherit the parent workspace's
feature selection. The default application uses only `bedtime-desktop`; `layout`
adds the document-independent `bedtime-workbench`, and `effects` adds
`bedtime-plugin-effects`. The workbench uses `bedtime-api` with document contracts
disabled. There is no bEd settings, editor,
workspace or remote dependency and no resource-directory discovery.

Build with `cargo build --manifest-path tests/fixtures/desktop-app/Cargo.toml`.
Run the resulting `desktop-consumer --smoke` executable from any working
directory. Native smoke verifies embedded default fonts, clipboard, command
activation, typed input, final-frame capture, appearance changes and shutdown.
The optional layout build also captures the panel through the same base API.

The listed local dependency patches are currently required for the renderer
and clipboard integration. Distribution of those patches is release work.
