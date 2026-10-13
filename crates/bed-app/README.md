# bed-app

This crate implements bEd's application policy: selected workspaces, MRU and
window handoff, SSH connections, startup restore, numbered-layout navigation,
workspace persistence and the application command reducer. Reusable desktop hosts
start with [`bed-core`](../bed-core/README.md).

bEd installs core in `HostRequestMode::Application`, then installs
`AppRuntimePlugin`. The application reducer owns its document requests and
native presentation, including remote save acknowledgements and workspace change
barriers. The binary retains configuration loading, helper/debugger dispatch,
CLI choices and smoke commands.

Feature plugins consume their domain inputs. `configure_feature_targets` maps
bEd's selected workspace to `ExplorerTarget`/`ExplorerDefaults`, `GitTarget`,
`DebugTarget` and HTTP persistence wording. Ordinary terminal panels use
`TerminalWorkspace`; `TerminalAppIntegrationPlugin` opts into bEd's shell/file-link
integration. Other applications can supply the same targets directly.

Features contribute state through `bed_api::features::FeatureStateContribution`
and emit `FeatureStateChanged`. These contracts carry only feature state. bEd
chooses the workspace storage location and preserves unrecognized feature blobs.
Restoring a workspace supplies domain targets before restoring feature state.

Numbered layout membership and retention live in
[`bed-plugin-layouts`](../bed-plugin-layouts/README.md). This crate supplies the bEd navigation
context, close/save preflight and existing persistence schema. Switching layouts
suspends outgoing panels, retaining views, local drafts and terminal processes.

`AppRuntimePlugin` supplies policy and request processing; the main binary
composes it with native projections through `BedAppPlugin`. The code editor is
the optional `bed-plugin-editor` feature, while `DocumentSession` belongs to
the shared `bed-documents` foundation. Workspaces and Settings are bEd-specific
panels; reusable hosts choose their own workspace and settings UI.
