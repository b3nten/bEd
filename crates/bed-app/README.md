# bed-app

bEd application policy sits above [`bed-core`](../bed-core/README.md): native window
handoff, startup decisions, directory/workspace transitions and numbered layouts.
The binary keeps configuration, helper/debugger entrypoints and smoke commands.

The application coordinates its ordered workbench requests with core's shared
editing owner. Drafts, edits, history, asynchronous saves and service polling run
through core. Context transitions hold an operation barrier while saving and
revalidate document revisions before retirement.

A live `FileContext` supplies the directory/provider for Documents, Search,
Explorer and Git. A saved workspace is optional. Unknown directories and remote
roots stay temporary until Save Workspace; their layouts/plugin state cause no
automatic disk writes. Saving attaches persistence without restoring an older
arrangement or replacing documents, undo, panels, suspended layouts or terminals.

The `workspaces` feature installs optional workspace lifecycle and commands.
`bed-plugin-workspaces` owns its contracts and an injectable repository; bEd's
adapter retains the version-2 `workspaces.json` format, file lock, atomic writes,
unknown plugin blobs and unvisited layout slots. Remote UI state remains local.
Without this feature bEd opens temporary directories and does not read saved
workspace state. Explicit legacy standalone restoration remains available.

Plugin snapshots use [`bed-storage`](../bed-storage/README.md), exposed through
`bed_api::storage`. Global preferences/layout templates remain independent of
workspace persistence. Numbered layout retention lives in `bed-plugin-layouts`.

Terminal launch context follows file context; plugin owners register shell
commands. Workspace integration contributes `+workspace`/`+w`, preserving their
calling-terminal promotion and window-handoff behavior. Save Workspace is a
separate operation.
