# bed-plugin-workspaces

Saved-workspace identity and persistence are independent of the live directory
and editing services. `WorkspacesIntegrationPlugin` installs lifecycle contracts
and registered commands; `WorkspacesPlugin` adds the picker panel.

Applications supply `WorkspaceRepositoryResource`. The default repository is
in-memory. `WorkspaceRepository::commit` saves the authoritative definition,
layout snapshot and namespaced plugin contributions. A separate recent-location
index can fail afterward; `take_index_error` reports that failure while the
workspace remains saved. bEd atomically writes each workspace snapshot. Saving
marks attachment saved only after the commit succeeds, and never restores
contributions or replaces live documents/panels. Unknown contributors and
unvisited slots survive.

An optional `WorkspaceLayoutSnapshot` supplies application layout serialization.
Opening another workspace requires a `WorkspaceOpenHandler`; the application
owns its context save/discard/cancel policy and native window launching. bEd sets
`WorkspaceLifecycleMode::Application` and admits `WorkspaceRequest` messages in
its shared ordered coordinator.

The integration contributes Save Workspace to the application menu and
`+workspace`/`+w` shell aliases. Directory/file access remains available without
this plugin. `WorkspaceAttachment::Temporary` carries no persistent identity.
