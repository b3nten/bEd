# bedtime-git

GUI-free typed Git operations through the installed Git executable. Call
`execute` on a worker or use the cancellable `Job` API. Git owns index locks,
hooks, signing, configuration and commit identity.

`Operation::Commit` takes a message and `CommitMode`: `Commit` creates an ordinary
commit, `Amend` replaces HEAD using the current index, and `Signoff` adds Git's
`Signed-off-by` trailer. Amend can change only the message when nothing is staged;
it never stages working changes implicitly. All modes reject empty messages and
unresolved conflicts and retain the normal Git hooks. Serialized requests that
omit `mode` retain ordinary commit behavior.

Run the shared repository regressions directly with:

```sh
cargo test --offline --locked -p bedtime-git
```
