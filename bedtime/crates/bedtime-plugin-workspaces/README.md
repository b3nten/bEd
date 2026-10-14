# bedtime-plugin-workspaces

Saved-workspace identity and persistence are independent of the live directory
and editing services. `WorkspacesIntegrationPlugin` installs lifecycle contracts
and registered commands; `WorkspacesPlugin` adds the picker panel.

Applications supply `WorkspaceRepositoryResource`. The default repository is
in-memory. `filesystem::WorkspaceStore` provides the optional filesystem backend
used by bEd, configured with `ApplicationPaths`. It stores local snapshots in the
selected project directory, remote snapshots on the client, and the recent-location
index in the application configuration directory. Its `repository()` and
`state_store()` methods provide the shared adapters. Constructing the store does
not promote a temporary directory or create workspace files.

`WorkspaceRepository::commit` saves the authoritative definition,
layout snapshot and namespaced plugin contributions. A separate recent-location
index can fail afterward; `take_index_error` reports that failure while the
workspace remains saved. The filesystem backend atomically writes each workspace snapshot. Saving
marks attachment saved only after the commit succeeds, and never restores
contributions or replaces live documents/panels. Unknown contributors and
unvisited slots survive.

`WorkspaceLayoutSnapshot`, `WorkspaceLayoutPreflight` and
`WorkspaceLayoutRestore` supply application layout serialization, validation
and restoration. Preflight runs before retiring the current session. The payload is opaque to the repository;
numbered slots and templates remain bEd policy. Text history is captured into
`WorkspaceSnapshot.history` rather than written to a separate history path.

Workspace requests enter core's ordered operation queue through barriers. Core
owns document decisions, save completion, context replacement and retirement;
the workspace lifecycle owner commits persistence before retirement. Failed
pre-commit writes keep the live session intact. Applications with no document
host can instead provide `WorkspaceOpenHandler`. `WorkspaceWindowHandler`
provides application process policy for a second window.

Install `WorkspacePersistencePlugin::new(repository)` to select an adapter;
explicit selection wins regardless of installation order. The included tests
exercise filesystem, memory and application-owned SQLite repositories. The
repository needs only recent/find/load/commit/forget/rename operations. The
filesystem adapter stores remote snapshots on the client and imposes no
numbered-layout schema. Unknown plugin contributions survive through the shared
storage snapshot collector. A failed restoration reports that the workspace
opened and preserves the saved snapshot from autosave and exit writes; explicit
Save Workspace can replace it once the application has recovered. Reconnecting
a live SSH context does not require reading or rewriting workspace storage.

The integration contributes Save Workspace to the application menu and
`+workspace`/`+w` shell aliases. Directory/file access remains available without
this plugin. `WorkspaceAttachment::Temporary` carries no persistent identity.

bEd's workspace picker and File → Open Folder create a saved workspace when
opening a new project, select layout 1, and record the project in recent workspaces.
Dropped folders and new directories opened from the command line remain temporary
until Save Workspace is requested.
