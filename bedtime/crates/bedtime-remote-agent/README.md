# bedtime-remote-agent

A remote helper executable for filesystem, Git, language-server and process
operations. It runs without a display server or GPU; `bedtime-remote` supplies its
client and protocol contracts.

```sh
cargo build --locked -p bedtime-remote-agent
./target/debug/bedtime-remote-agent --protocol-version
./target/debug/bedtime-remote-agent --stdio
```

Desktop packages bundle static Linux x86-64 and ARM64 agents. SSH connection
setup copies `remote-helpers/<target>/bedtime-remote-agent` into a private,
content-addressed remote cache and verifies its protocol before using it.
`BED_HELPERS_DIR` selects the local bundle directory. Existing serialized custom
agent paths remain literal paths; the application does not rewrite them.

Build both bundled targets with `scripts/lib/build-remote-helpers.sh`. Stage or
validate bundles with `scripts/lib/remote-helpers.py`. The standalone command
packages the host-native release executable and licenses into
`target/remote-agent-package.zip`. Bundle manifest format and protocol versions
remain independent of the executable's name.
