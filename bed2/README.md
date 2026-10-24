# bEd 2

A workbench application with a temporary default workspace rooted at the user's
home directory. An explicit directory argument or **File → Open Folder…** opens
or reuses a folder workspace. Application metadata lives in the user configuration
directory, never inside the opened project.

The titlebar controls are **Terminal, Git, Files, Structure, Settings**. Click to open
or focus a tool in the current layout; drag to create a new panel in an area or
along its edge. **View** also offers concrete panel creation. Workbench has no
panel-type registry or picker.
The controls match bed1's native toolbar geometry: 16-point artwork in 26×22-point
buttons, 2-point gaps and a 10-point trailing inset. UI2 owns the macOS final alpha
conversion so antialiased edges remain smooth over transparent window backgrounds.
Toolbar icons use the theme's text color through hover and press, with animated
interaction backgrounds.

Settings has General, Appearance, Editor and Language servers sections with a Global/Workspace
switch. Startup opens a single terminal layout by default; General can opt into
reopening the last folder workspace. Document autosave defaults to a one-second idle delay. Five numbered layout
slots retain live views and workers; their starting arrangements can be saved as
global defaults. See the [composition record](../MIGRATION/application-composition.md)
for boundaries and the remaining Save As/recovery and workspace-management work.

```sh
cargo run --offline -p bed2
```

`src/main.rs` installs Bevy, UI2 and Workbench. Startup attaches `WorkbenchSurface`
and `Titlebar { height, draw }` to the window and spawns an explicit
`WorkbenchCamera`. `settings.rs` owns explicit global values, individual workspace
overrides and their application to UI/editor/tree state. `storage.rs` owns global
RON preferences I/O; `session.rs` owns one combined file per folder workspace.
`layouts.rs` creates concrete layouts after settings/session readiness and keeps
live instances while switching slots. Required components supply panel metadata
and drawing immediately; there is no panel registry or delayed panel initialization.
`menus.rs` owns creation choices, and `main.rs` renders the titlebar.
`cube.rs` owns the shared cube/ground/light scene and viewport implementation;
`tasks.rs` adds the Tasks & Processes panel. `bedtime-terminal2::TerminalPlugin` adds local shell
panels; `terminal.rs` owns `+open`/`+o` integration and optional automation inspection.
`opening.rs` owns file/folder opening, viewer selection, tab reuse and area
placement. `files.rs` installs the file tree and the current file viewer; `file_editor.rs`
owns their canonical document bindings and save backend. `code.rs` supplies
canvas context-menu actions, analysis execution and the application-owned
[Structure viewer](crates/bed-structure2/README.md).
`close.rs` owns application quit decisions and workspace worker shutdown.
`git.rs` owns local repository operations and document coordination;
`git/panel.rs` renders the Git panel in egui.
All use the main World and renderer. Open another panel through **View**; empty
areas no longer discover registered types. A concrete panel spawn selects its tab
without changing area focus. `select_panel` and `focus_area` are independent;
`focus_panel` explicitly does both. Workbench hooks maintain ownership and fallback
selection/focus immediately as composition applies.

The memory-only Documents and Hello World demos remain removed. Files and terminal
helpers share the application opening route; opening a file needs no Files panel.
The standalone library examples remain available independently.

## Bed mascot

**View → Bed** creates an interactive mascot panel supplied by
[`bedtime-mascots2`](../bedtime2/crates/bedtime-mascots2/README.md). Move the pointer
to turn the mascot and press the primary button for a squash and sound. Bed's
patterned blanket follows the current theme accent; reduced motion freezes the
pose while retaining click audio. Audio is opened lazily and device failure
leaves the visuals available.

Panels have independent state and cameras in the application's Bevy world.
Hidden tabs, layouts and workspaces pause rendering and animation; closing them
retires scene entities, image handles and playback. Sessions and saved layout
defaults preserve kind and placement, with fresh animation and playback on restore.
The startup layout and titlebar remain unchanged.

## Model viewer

GLTF, GLB and STL files open in **Model** through Files, file dragging and
Terminal's `+open`, using the shared opening route. Open With retains Code and
Hex alternatives. Preview/pinning, focus and placement follow ordinary file
viewers; there is no empty model tool or titlebar button.

The model canvas fills the entire panel without padding. Buttons overlay its
top-left corner, and the smaller interaction hint overlays the bottom-right.
The viewer ports bed1's Frame All, orbit/pan/zoom, Info, display modes, normal
vectors and Appearance controls, including all five lighting environments,
skybox blur/horizon, shadows, AO and exposure. Primary+Plus/Minus zoom and
Primary+0 reframes the focused model. Animation counts and authored skeleton
poses are supported; playback and model editing remain deferred. Rendering uses
[`bedtime-model2`](../bedtime2/crates/bedtime-model2/README.md) in the application's
existing Bevy world, with isolated scenes and hidden-view camera suspension.

Textual `.gltf` views share canonical documents with Code and refresh from unsaved
edits. GLB/STL sources are read-only snapshots. Relative local buffers/textures,
including `../` paths and symlinks, resolve from the model directory; URLs and
absolute URI paths are rejected. Sidecars use saved disk contents, with bounded
worker reads and one-second dependency checks while visible. Returning to a view
resumes checks; changed root sources use the existing file observation flow.
Confirmed moves rebase relative resources without losing canonical text edits.
Preparation and asset-loading failures show details while retaining the last
valid scene. Different files start freshly framed; same-source reloads retain
camera and appearance. Folder workspace sessions restore those values per tab;
older session records default the new optional field. No bed1 session import runs.

Verify with the focused crate/application commands in the model README and
`python3 -B bedtime2/tools/smoke_models.py` after building with `automation`.

## Git panel

Open **Git** from the titlebar or **View → Git**. Its layout follows bed1:
branch selection/creation and Fetch/Pull/Push/Refresh across the top, a
virtualized Conflicts/Tracked/Untracked file tree, and a fixed commit composer with
operation progress/cancellation below. List mode shows filenames with muted parent
directories. Their checkboxes represent unstaged, partially staged and staged
content; section checkboxes stage or unstage that section. Newly added files
stay in Untracked until committed when grouped by tracked/untracked. Commit
requires staged changes, a message and no unresolved Git conflicts. The centered submit button has an adjacent menu for
**Commit**, **Amend**, and **Signoff**. Amend replaces HEAD with the current index
and entered message, including message-only amendments when nothing is staged;
working changes are not staged implicitly. Signoff adds Git's `Signed-off-by`
trailer using the configured identity. All modes use the same managed operation,
reviewed-status check and Git hooks. The action choice is workspace runtime state.

The small settings button beside the branch picker selects **List** or **Tree**
and groups by **None**, **Tracked and untracked**, or **Staged and unstaged**.
Tree is built from Git's changed paths, so deleted files remain visible.
Expandable folders contain their changed files. Files and Git share UI2's tree row painter and motion primitives: the same
row height/indentation, filename artwork, item-sized hover/selection backgrounds,
rainbow selection text, rotating disclosures and smoothstep movement/fading.
Reduced motion settles immediately. Git retains status-colored file icons and
its staging checkbox column. Folder checkboxes act on all changed descendants,
including hidden descendants of collapsed folders. Grouped views retain a separate Conflicts
section; ungrouped views keep conflict markers on their file rows and disable
folder staging when descendants contain conflicts.

In staged/unstaged grouping, partially staged files appear in both sections.
The staged copy unstages that file; the unstaged copy stages its remaining
changes. The commit count still counts each staged file once. View/group choices
are shared by the workspace's Git panels and persisted with its session. New
workspaces and older sessions without a saved choice default to Tree with
tracked/untracked grouping; explicit saved choices survive. Folder expansion is
runtime state. Changing settings or expanding a folder prepares rows from the
cached immutable status on a managed worker, without another Git query; stale
preparation cannot replace a newer status or choice. Tree row indexes are also
prepared on the worker. Each panel retains motion only for its recent viewport;
collapsing a large folder retains only previously drawn descendants for fading.
Reopening during a collapse reverses from the displayed position/opacity. Scroll
anchors survive row shifts; only an explicit folder collapse can replace a lost
anchor with its visible parent.

Single-click selects a file. Double-click or Enter opens it through the normal
file-opening route. The context menu offers Open file, Stage/Unstage and confirmed
Discard where applicable. Conflicts offer current/incoming/delete choices and
Save and stage resolution; Git refuses to stage remaining conflict markers.
Continue/Abort appear during supported repository operations. Delete and Abort
require confirmation. Text diff views, hunk controls and ordinary editor/tree
Git decorations remain deferred.

Repository state, the commit draft and admitted operations belong to the
workspace, shared across panel instances and layouts. Closing a panel preserves
operations; closing its workspace requests cancellation and drains the backend.
Successful commit clears only the message actually submitted, preserving a newer
draft. Folder sessions persist Git placement and the commit draft in the existing
centralized RON record; temporary workspaces remain memory-only. Older records
default the new draft field to empty. Drafts are limited to 64 KiB at the session
validation boundary.

The application reuses the GUI-free `bedtime-git` backend and the installed Git
executable, including its hooks, signing and configuration. A subdirectory
workspace discovers and displays its parent repository within scope, but requires
**Open repository root** before writes. Authentication requiring terminal input
can be completed with **Open Git terminal** after an error. This is local Git
integration; it does not install bed1's SSH workspace infrastructure.

All Git and source reads run in managed background tasks. One mutation is
admitted per workspace, redundant refresh requests coalesce, and stale scan
epochs cannot acknowledge newer operations. Status scans run every two seconds
with a visible panel, ten seconds with hidden panels, and stop periodic polling
after the last panel closes. Reopening requests fresh status. File groups, order,
counts and the single-file message hint are prepared on the worker; drawing only
visits the visible rows. Filesystem-triggered refresh is a later optimization.

Staging first saves relevant canonical documents and rechecks revision/source
identity before dispatch. Destructive actions refuse dirty, saving, composing or
conflicted sources and recheck after preparation. Process-local write leases
exclude competing editor saves across overlapping workspace roots, remain alive
through worker completion and source reconciliation, and defer autosave while
held. Git changes reach open and retained text documents through the existing
source-generation gate, preserving local edits and undo. Confirmed discard uses
the backend's displayed-snapshot check. Commit also checks the last reviewed
status before starting. These checks do not turn external Git processes into an
atomic compare-and-swap protocol.

Zed's [repository job scheduling](https://github.com/zed-industries/zed/blob/main/crates/project/src/git_store.rs)
and [Git panel](https://github.com/zed-industries/zed/blob/main/crates/git_ui/src/git_panel.rs)
were references for ordered work, coalesced scans, pending-state handling and
virtualized rows. bEd2's coordinator stays application-owned because file
identity, save policy, opening and persistence belong here. The reusable backend
remains independent; no viewer registry or diff API is installed by this panel.

Verification:

```sh
cargo test --offline --locked -p bed2 --features automation
cargo test --offline --locked -p bedtime-git -p bedtime-ui2 -p bedtime-filetree2
cargo build --offline --locked -p bed2 --features automation
python3 -B bedtime2/tools/smoke_git.py
```

On 2026-10-23, all 137 ordinary application tests passed (two opt-in tests left
ignored), including 25 Git integration/session/presentation regressions. All 20
backend, 100 Files tree and 30 UI2 tests passed. The shared backend regressions
now run directly through `cargo test -p bedtime-git`. Native macOS/Metal automation
passed eleven nonblank Git captures covering all six view/group combinations,
shared Files/Git row height, aligned checkboxes, bounded viewport motion state,
directory collapse/stage/unstage, side-specific partial staging, 502 changes,
centered commit labels, actual Commit/Amend/Signoff operations, confirmed discard,
branch creation, narrow resizing and restoration of view/group choices and the
commit draft. A 30-sample unoptimized local run measured about 0.22 ms median for
the panel drawing function in default Tree mode with that fixture. This excludes
whole-application/GPU frame time and background Git work. Captures and the timing
summary are in `target/git-smoke`. The Files native regression also passed eleven
captures after sharing the painter, including rename, system clipboard cut/paste,
watching, injected drops, restoration and a 10,000-entry tree with 23 painted
viewport rows (`target/shared-tree-files-smoke`). Native Linux UI was not exercised.

## Local packages and installation

On macOS, build a release app and install it for testing:

```sh
scripts/package-bed2-macos.sh --install
open /Applications/bEd2.app
```

The script creates an ad-hoc signed `target/dist/bed2/bEd2.app` and
`bEd2-<version>-<architecture>.zip`. Installation stages and verifies the new app
before replacing `/Applications/bEd2.app`, restoring the previous app if the
replacement fails. Its bundle ID is `org.bed-editor.bed2`; bed1's `bEd.app` and
settings remain separate. Omit `--install` to build packages only.

Use `--skip-build` to package an existing `target/release/bed2`.
`BED2_BINARY`, `BED2_DIST_DIR` and `BED2_INSTALL_DIR` override the executable,
package output directory and macOS installation directory. For a user-local
installation, use `BED2_INSTALL_DIR="$HOME/Applications"`.

On Linux:

```sh
scripts/package-bed2-linux.sh
```

This produces a `bEd2-<version>-<architecture>-linux.tar.gz` and, when `dpkg-deb`
is available, `bed2_<version>_<architecture>.deb`. The archive contains a `usr/`
tree with `bin/bed2`, a distinct desktop entry and icons, and notices. Extract
with `--strip-components=1 -C /usr/local` to install its contents under
`/usr/local`, or install the Debian package with the system package manager.
Linux packaging requires ImageMagick and the normal desktop build dependencies.

Both scripts build only `bed2` and package its embedded-asset license notices;
no SSH helpers or Docker builds run. macOS packages are for local testing and
are not notarized. Run packaging and license regression checks with:

```sh
python3 -m unittest discover -s scripts/tests
```

Verified on 2026-10-21: release build, real macOS app/ZIP assembly, signature
verification and `--install` into an isolated temporary Applications directory
passed. All 34 script tests passed, including Linux archive assembly and failed
installation rollback fixtures. Native Linux packaging was not exercised.

## Files and documents

On macOS the application installs real native **bEd**, **File**, **Edit** and
auxiliary **View** menus:

- **bEd → Quit bEd**;
- **File → Open Folder…, Save**;
- **Edit → Undo, Redo**, a separator, then **Cut, Copy, Paste**;
- **View** creates Files, Structure, Terminal, viewport, tasks and Settings
  panels; it never creates an empty Code panel.

The same abstract definitions supply an in-window fallback where native menus are
unavailable. Standard editing accelerators use the focused canonical editor or
text widget, not a second undo history. Save/Open Folder/Quit bindings are app-owned.
The constrained menus expose no New, Save All or Reload actions. Source conflicts
retain their inline recovery controls; Save As remains unfinished.

Click **Files** in the titlebar to open or focus the tree in the current layout.
With no argument its root and default terminal cwd are the user's home directory,
not the executable's launch directory. **View** creates auxiliary panels, not Code:
open Code by opening a file from Files. An explicit directory chooses a folder root:

```sh
cargo run --offline --locked -p bed2 -- /path/to/project
```

The code editor has no top control bar. Its canvas context menu contains only
Cut, Copy and Paste by default. A ready language server can contribute Go to
Definition when it advertises that capability. Titlebar tool icons
animate their background and ink on hover and show descriptive tooltips.

Single-click opens a replaceable preview beside Files, updates Structure to that
file, and retains tree focus.
Double-click or Enter pins the tab and focuses the editor. Modified/drafting/saving/conflicted documents are never replaced by another
preview. Reopening a file within a workspace shares its canonical document,
including retained edits/history after its last viewer closes. Different layouts
have separate preview slots. Additional shared views can be created by dragging a file from Files; the
canvas menu does not expose view creation.

UTF-8 files up to 256 MiB load on managed background workers. Binary, oversized and
nonregular files show metadata. No synchronous full-file reads occur while drawing.
**Save** or Cmd/Ctrl+S uses the canonical save service, source checkpoints and atomic
replacement. A changed disk source refuses overwrite; **Reload to compare** in the inline conflict controls compares incoming
bytes while preserving local edits, then **Keep my edits** / **Use disk contents**
resolve an observed conflict. Confirmed tree moves preserve document/history and
rebind saving to the new path. Deleted dirty sources retain edits and lose their
save binding; Save As and missing-source recovery remain future application work.

The restored tree includes lazy watching, file operations, per-panel settings and
root-scoped visibility. The application stores root-scoped visibility in the owning folder workspace's
config file, together with its settings and layout state. One asynchronous writer
owns the entire workspace record; admitted changes survive workspace retirement.
The default home workspace retains visibility changes in memory, including when
home contains an existing `.bed` directory. The reusable plugin chooses neither
cwd nor storage itself. Core Workbench snapshots/restoration remain removed; application session recipes
are described below.

Files supports dragging a single file into an area, tab strip or edge split using
Workbench's titlebar creation targets. Each drop creates a pinned viewer at that
placement and shares canonical content/history with other viewers of the file.
Dropping onto an eligible folder in Files retains the existing move/copy behavior.

## Image viewers and Open With

Image files open in the toolbar-free canvas supplied by
[`bedtime-image2`](../bedtime2/crates/bedtime-image2/README.md), centered and
contained in the panel. Wheel and pinch zoom at the pointer, primary-button drag
pans, and double-click returns to fit. PNG/APNG, JPEG/JFIF, SVG, GIF, WebP, BMP,
ICO, TIFF, TGA, PNM variants, QOI, DDS, HDR and EXR are supported; animated formats
show their first frame. Transparency uses the normal panel background.

`opening.rs` owns an explicit association table with stable viewer IDs,
case-insensitive extensions and descending priorities. Image has priority 100 for
its extensions; Code has priority 0 as the universal text/metadata fallback.
Files, Git, terminal helpers and file drops all use that policy. Automatic opens
resolve the default before looking for an existing same-file/same-viewer tab in
the origin layout. A safe temporary preview can change viewer kind. File drops
always create a pinned viewer at the requested placement.

**Open With** appears in the file canvas and Files context menus when multiple
viewers are supported. It lists viewers by priority and marks the default.
Choosing another viewer opens/focuses a separate pinned tab; it preserves the
originating tab. New tabs from a canvas appear beside it in the same area.
Files uses normal viewer placement. Choices apply to that opening; there is no
saved association preference or settings UI. Existing sessions retain their
recorded Code viewers, including SVG files; new Image tabs restore as Image.
Zoom/pan is runtime-only and starts at fit after restoration.

SVG Image panels bind the same canonical text document as Code, without a hidden
editor. Committed unsaved edits update the image after a 150 ms settling period.
Invalid intermediate source retains the last valid image and shows an error.
Save/conflict/autosave/close policy remains canonical, and dirty shared SVGs
protect temporary previews from replacement. Source-generation and revision
checks reject obsolete read/render results. Raster images are read-only source
snapshots. Background source observation refreshes image viewers after disk
changes, including outside the tree root; confirmed moves and deletion follow
the common file lifecycle. SVG rasterization is refreshed for physical display
size within a 64 MiB output budget. Hidden views defer resolution/source rendering
until visible after their initial load; closing/replacing cancels pending work
and releases textures. Encoded source reads are bounded to 256 MiB.

Verification: `cargo test --offline --locked -p bed2 --features automation`,
`cargo test --offline --locked -p bedtime-image2 -p bedtime-filetree2`, and native
`python3 -B bedtime2/tools/smoke_images.py` after building with `automation`.

## Opening from the terminal

Local terminals provide `+open` and `+o` through Terminal2's optional shell-command
API. Main handles helper invocations before constructing Bevy or initializing a
window/renderer. Helpers are private symlinks to the running executable, added to
child PATH; no separate helper binary is packaged or installed globally.

- `+open` opens the calling shell's current directory as a folder workspace,
  activating an existing workspace for its canonical root or creating one and
  loading its stored session. The temporary home workspace is retained.
- `+open .` or one explicit directory uses that same folder-opening path.
- `+open FILE ...` resolves paths relative to the calling shell's actual cwd,
  including after `cd`, and opens pinned tabs in the originating workspace/layout.
  `+o` is an alias with identical behavior. Supply directories separately from files.

`opening.rs` is the shared route for file-opening intentions. Files validates its
source panel/root and supplies preview/pin/focus intent; shell commands supply
validated runtime origin and paths. The route owns viewer selection, same-layout
reuse, protected preview replacement and placement. Default image associations use Image; other files use the
text/metadata viewer. Association and area rules belong here, not in the tree or terminal.
An existing viewer area is preferred; otherwise use the largest area without Files,
splitting only when every area contains a tree. Reopening a file preserves its
canonical document and unsaved edits.

File helpers acknowledge after loads and document binding finish; missing files,
closed origins and load failures return a nonzero exit status with a diagnostic.
Folder helpers acknowledge validated workspace activation; normal session restore
continues through the existing loading/error UI. Switching focus/workspaces cannot
redirect an accepted file open. Commands respect quit and workspace readiness gates.
The shared transport times out after 60 seconds without automatically retrying or
rolling back an accepted operation.

Verified on 2026-10-22: 99 application tests passed (the native GPU benchmark was
left ignored), along with 47 Terminal2 tests and 11 shared transport tests. The
native macOS/Metal `smoke_open.py` scenario passed two captures, exercising real
helper dispatch, cwd workspace creation/reuse, relative paths with spaces, binary
metadata, alias/reopen behavior, error exit status and saved-session restoration
through no-argument `+open`. Native Linux was not exercised.

## Code and Structure

Open a file through **Files**, then choose **Structure** from the application's
**View** menu. File extensions choose the initial language. Library hosts can
share canonical documents across views with independent selection, scrolling,
wrapping and collapsed regions. Gutter arrows fold/unfold regions and also work
with **Line numbers** hidden. Syntax colors
follow the live theme. Plain text remains editable without structural analysis.

Structure uses the current eligible editor or remembers the last one while
Structure/Terminal/other panels have focus. It retains the last same-source
outline while analysis refreshes, so editing does not flash the empty-state message.
Automatic tab focus after closing a
source may establish another eligible source. Otherwise a closed/rebound source
shows **No structure available**, without scanning previous editors. Clicking a
declaration reveals its actual source tab/layout, unfolds its name, selects it
and focuses that view. Stale or composition-blocked activation has no effect.
The [Structure contract](crates/bed-structure2/README.md) and
[editor APIs](../bedtime2/crates/bedtime-editor2/README.md) describe ownership.

The editor's `treesitter` feature proves external contributions through shared
observations. The source viewer is application-owned and code-only; no generic
structure API is introduced. Analysis shares one bounded managed worker per
viewed document and retires after its last editor closes. Closing viewers retains
the canonical document under Workbench's existing contract. Files reopens retained
file documents; folder session recipes reopen file descriptors, not unsaved contents.

## Language servers and status bar

Open a folder and a supported code file to start its configured local language
server. Defaults use `rust-analyzer` for Rust, `gopls` for Go,
`typescript-language-server --stdio` for TypeScript/TSX and
`pyright-langserver --stdio` for Python. Servers must be installed separately.
**Settings → Language servers** enables/disables language
tools and edits configured programs/arguments. **Apply** commits a server draft
and restarts affected processes; typing in a draft does not restart on each key.
Settings inherit Global/Workspace values and use the existing centralized RON
`.bed` stores. The home workspace does not launch project servers. Existing saved server maps
are preserved; **Add … support** fills missing built-in definitions explicitly.
TypeScript and TSX defaults share a process; their launch configurations can be
customized independently.

Settings has a 14-point inset around its header and navigation. The header shows
its active configuration file beside the Global/Workspace toggle in a single row.
The path truncates to the remaining width, shows its full value in a tooltip and
copies on click; temporary scopes show **In-memory settings**.
Narrow panels move sidebar navigation into a wrapping row above the scrollable
content; long paths truncate without displacing the scope switch.
Option rows place controls before their names and center their contents vertically.
Boolean options use label-clickable switches; Theme precedes the font controls,
and separators group startup/autosave, theme/fonts, window, panels, tree and effects.

Current features are document synchronization, push diagnostics, definition
navigation, symbol hover and completion. Diagnostics draw revision-bound underlines and message tooltips.
**F12** or the canvas menu's **Go to Definition** resolves the caret, opens the
destination through the common Files route and selects/reveals its range.
Multiple targets present a chooser. Destinations use temporary preview tabs:
another clean preview can be reused, editing or explicit tab activation pins normally, and an
already pinned destination stays pinned. A source preview can itself be replaced;
the accepted navigation retains its ticket/workspace and validates the target
load stamp and revision before applying. Closed/rebound/newer requests cannot
deliver late selection changes. Conversion for long target lines runs off-thread.

**Cmd-click** (Ctrl-click on Linux) resolves the symbol under the pointer without
first moving the caret. Modifier-hover underlines symbols with the live theme accent and uses a link cursor.
It shares F12's chooser, preview policy and revision/generation gates.

Symbol hover waits **200 ms** before requesting information. Ordinary UI tooltips
remain instant. The input-capable popup combines documentation and current
intersecting diagnostics, remains readable under the pointer, scrolls vertically,
and dismisses with Escape. Bed1's presentation is the reference: compact text,
wrapped signatures, themed code highlighting, bold/inline-code prose and rules.
Both symbol and completion documentation reserve at least six text lines when
scrolling is needed, bounded by the available screen region; shorter text shrinks.
Documentation is parsed once per payload; a panel-owned managed Tree-sitter worker
prepares fenced-code colors. It does not install the bed1 renderer/runtime.

Completion starts on identifier typing, negotiated trigger characters and
**Ctrl-Space**. Arrow keys select, Enter/Tab or a click accepts, and Escape closes.
Lists preserve server sorting, filter the identifier prefix and refresh incomplete
results. Item details/documentation resolve lazily; accepting an unresolved item
waits for its additional edits. A revision-checked editor transaction applies the
replacement and auto-import edits together, with one undo step. Supported snippets
have numbered/default/choice placeholders, mirrors, escapes and variable defaults;
Tab/Shift-Tab traverse placeholders. Transform expressions are unsupported, and
unusable items are omitted. Server completion commands and commit characters are
not executed. Composition/read-only gates remain authoritative.

Hover/completion/resolve have independent per-view tickets and cancellation; they
cannot supersede definition navigation. Explicit ContentModified/ServerCancelled
responses retry at most three times (100/200/400 ms) within the original deadline,
using the same captured source. Edits, detachment and superseding requests cancel
obsolete work; this is request retry, not automatic process restart.

Empty hovers are not permanently cached across accepted diagnostic reports,
completed work or rust-analyzer project-status changes. The client handles
`experimental/serverStatus`; empty hover/definition responses received while that
server reports loading wait for it to settle, under the original request deadline.
The Language Servers panel exposes project loading/health separately from process
readiness. A real native regression covers `Vec`/`Vec::new` hover and selects the
actual standard-library method in a temporary tab.

`status_bar.rs` installs the application's renderer in Workbench's existing
workspace status slot. It displays startup/ready/failure state, current-document
nonzero error/warning counts and document/save errors. It uses focused text or the application's remembered Structure source
while Files/other tools have focus. A compact diagnostics icon opens a menu with
one submenu per loaded server, containing only Restart and Stop, plus Open Language
Servers. Stop uses managed process cleanup and suppresses relaunch until explicit
Restart or a changed launch configuration; shared-language bindings stop together.
The menu remains reachable after stopping the last server. Server failures
elsewhere in that workspace remain visible.
Counts are cached with the report and never scan the source or diagnostics while
drawing. Stale reports do not count as current errors. Without LSP state or a
document error, the renderer is absent and the shell reserves no status height.
The renderer tolerates the empty workspace before restoration creates a layout.
Document/save/navigation failures display their first nonempty message line,
truncated to fit the row, with the full error in a tooltip on that message. Notices
expire after eight seconds; this dismisses the notice without clearing canonical
save/conflict state. New navigation attempts can report the same error again.
Server failure details are exposed on the server-error count tooltip. Controls
share centered vertical bounds, and activity artwork uses the menu icon's size.

Other useful context actions in the [LSP specification](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/specification.md)
include Find References, Go to Type Definition/Implementation, Rename and Code
Actions. These remain candidates, not implemented menu entries. Location queries
can reuse revision-checked navigation; Rename and Code Actions first need checked
workspace-edit application across documents. Hover and completion already use
pointer/typing or keyboard interactions, so they add no extra context menu entries.

**View → Language Servers**, or **Open Language Servers** in the status menu,
opens the application-owned `LspPanelPlugin` panel. It shows configured/inactive
integrations, process/server identity, roots, negotiated synchronization/encoding,
attached documents, outstanding requests, work progress, recent server messages
and stderr, with managed restart controls. Panels persist in workspace sessions
and layout defaults; closing an observer does not stop workspace language workers.

Server activity and diagnostic freshness are independent. The bar spins for
startup or actual `$/progress` work, showing its title and percentage. Begin,
report and end tokens are tracked per process, including simultaneous numeric and
string tokens. The idle row contains only its menu icon; zero/unknown counts,
missing/stale report labels and the document title stay out of the row. Diagnostic
freshness and server lifecycle remain available in the panel. Neither a
missing nor stale report implies background work. An unfinished operation remains visible, with its age in
the panel after 30 seconds without an update; the client never invents an end.

`lsp.rs` projects canonical document URIs/language IDs and folder settings into
the optional [LSP2 plugin](../bedtime2/crates/bedtime-lsp2/README.md). The plugin
uses one managed process per root/identical launch configuration, shared roots,
ordered journal edits, negotiated UTF-8/UTF-16 and generation/ticket checks.
Hidden and inactive views keep syncing; clean viewless documents detach and
dirty ones retain synchronization. bEd2 pairs a worker's actual saved root with
the successful Workbench receipt for `didSave`, including saves of older edits.
CAS/conflict/source-changed failures do not emit save notifications. Workspace
quit waits for protocol shutdown, process-group termination and actual reaping.

The persisted `language_servers` setting is a map from protocol language IDs to
`ServerConfig`; a workspace override replaces that map as one setting value.
`language_servers_enabled` is independently inherited. Other server definitions
can be configured through `.bed` preferences or `app.set_settings` automation.
For example, a Python entry can use program `pyright-langserver` with arguments
`["--stdio"]`; executables must already be installed. Server settings and
initialization options are JSON values; Rust's defaults are an empty settings
object and null initialization options. A complete launch definition round-trips
literal arguments and environment overrides without invoking a shell.

The first slice is macOS/Linux local stdio only, with an 8 MiB source limit and
explicit queue/frame/result limits. Larger files keep plain editing and do not
launch language workers. The folder root is supplied by the host; nested marker
discovery and multi-folder sessions remain pending. Signature help, references,
pull diagnostics, filesystem-watch registration, formatting, rename, code actions,
workspace edits and remote transport are not implemented. Current syntax/folds
continue to use Tree-sitter independently. The removal of full-source retry-cache
copies is structural; no comparative frame-time speedup has been measured.

```sh
cargo test --offline --locked -p bedtime-lsp2 --features workbench
cargo test --offline --locked -p bedtime-lsp2 --features workbench real_rust_analyzer -- --ignored --nocapture
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_lsp.py
python3 bedtime2/tools/smoke_lsp_interaction.py --real-rust
```

The native scenario uses an isolated config/project and deterministic server to
exercise diagnostic rendering, F12 cross-file previews, saved captures, restart,
status errors, progress/report completion and observer lifetime through managed
quit. `app.inspect_lsp`, `app.inspect_lsp_servers` and `app.inspect_status_bar`
expose runtime/presentation observations; `app.inspect_lsp_interaction` also
reports hover/completion state, popup bounds and documentation syntax readiness; `app.lsp_definition` and
`app.lsp_restart` exercise the same controller entrypoints as user actions.

The backend was written for v2, using bed1's behavior and regression cases as
references. It does not depend on the bed1 LSP runtime or Helix. Bed1's
[Helix attribution](../NOTICE) covers its adapted language catalog and
configuration/root-selection approach, while transport/document ownership stayed
local. V2 currently supplies its own small configuration map, without importing
that catalog. The [Zed review](../MIGRATION/lsp2-investigation.md#status-and-robustness-review)
records the progress/observability changes and remaining reliability work.

## Viewports

Each viewport panel is also its `Camera3d`, with an independent `UiViewport` image
and `OrbitCamera` state. Left-drag orbits, Shift-left-drag or middle-drag pans,
scroll zooms, and **Reset view** restores the initial camera. egui owns the drag
until release, including outside the image; controls respect window focus and
popups. Panning scales with distance, field of view and viewport height. Pitch
and distance are clamped. Camera transforms update in `Update`; visibility is
synchronized in `UiSystems::SyncViewports` before UI2 resizes images and Bevy
updates projections. Hidden views stop rendering. Closing a panel retires its
camera and image ownership; the application-owned scene remains available.
No core layout/session serializer persists viewports. Any camera-state restore
recipe belongs to the application; the current session format does not store orbit state.

The Tasks & Processes panel demonstrates the existing `WorkspaceWork` API:

- **Start task** sums 1 through 100 on a worker thread, with a configurable
  duration, progress, a typed result (5050), and cooperative cancellation.
- **Start process** launches a fixed interactive echo script (`/bin/sh` on Unix,
  PowerShell on Windows). Send text with the button or Enter; `stderr` writes to
  stderr, `fail` exits with code 7, and `exit` exits successfully. **Close stdin**
  delivers EOF after queued writes; **Stop process** requests cancellation.
- Each panel retains its latest task result and the newest 64 KiB of each output
  stream. Process supervision status and the child's exit status are displayed
  separately: `Finished` can accompany a nonzero exit code.

Both workers are owned by the workspace where they started, with the panel as a
shorter-lived owner. Workbench handles cancellation and process cleanup when
either closes. An ordinary Update system collects results and drains output for
hidden panels and inactive workspaces too. Worker timing uses real time, not
Bevy's virtual time. No session serializer stores live workers, logs or results;
applications reconstruct fresh idle panels if implementing restore.

## Terminal

Choose **Terminal** from the application's **View** menu. Each panel owns one local
shell, started automatically after its first size observation. Normal shell input,
ANSI/truecolor output, wide/combining cells, scrollback, selection/copy, paste,
alternate screen, mouse reporting and resize use the headless terminal core with
a new egui canvas. The terminal follows the active theme's font size, default and
ANSI colors, with bed1's Everforest palettes for dark/light presets. Changing the Settings theme recolors existing indexed output without restarting
the shell. The library also supports application-supplied ANSI palettes. Explicit truecolor and shell
OSC overrides remain intact. Bedfont bold uses the actual bold face rather than
an overstrike; faint suppresses bold, and the grid/text align to physical pixels.
Tabs follow the local foreground process (falling back to the shell name), and
return to the shell when a job finishes. Names are observed off the UI thread,
including for quiet or hidden sessions; shell OSC titles remain separate metadata.
The running canvas has no extra title/Stop toolbar beneath its normal panel tab;
it uses a 14×12-point content inset with an inset scroll indicator. The default
background inherits panel opacity without adding another background layer.

- Copy/paste/select-all use Cmd+C/V/A on macOS and Ctrl+Shift+C/V/A elsewhere.
  Ctrl+C remains shell interruption. Shift bypasses application mouse reporting
  for local selection/scrolling; Shift+PageUp/PageDown/Home/End navigate scrollback.
- Shell exit retains the transcript and shows its status. **Restart** creates a
  fresh session after the old runtime has finished cleanup. Applications can still
  use `TerminalPanel::stop()`; the canvas has no permanent Stop control.
- Hidden/inactive sessions continue draining/parsing output, even with workspace
  background updates disabled. Panel/workspace closure and the normal quit flow
  cancel and drain the workbench-owned runtime off the UI thread.
- There is no core workspace/layout snapshot. Application folder recipes can
  reconstruct a terminal panel, which starts lazily, never from a persisted PID or
  transcript. Startup waits for root/preferences readiness, and uses home for the
  default workspace or the explicit folder root (unless launch policy is overridden).
  Local `+open`/`+o` helpers use the pre-startup dispatch described above.

Initial support is macOS/Linux only, without SSH, inline images or
shared debugger sessions. Fonts determine glyph coverage; cross-cell contextual
shaping, bidi reordering and color emoji are not implemented. The text-only parser
also omits DCS protocols and OSC 1337. See the
[terminal contract](../bedtime2/crates/bedtime-terminal2/README.md).

## Settings, layouts and persistence

All application metadata uses versioned RON with a `.bed` extension. The config
root is `$XDG_CONFIG_HOME/bed2`, falling back to `$HOME/.config/bed2`:

| File | Contents |
| --- | --- |
| `preferences.bed` | Global settings, startup policy/last folder and five saved layout defaults |
| `workspaces/<id>.bed` | One folder workspace's settings overrides, file visibility, layouts and active slot |

The workspace ID is the BLAKE3 digest of its canonical folder path's native encoded
bytes. Each record includes the root and checks it before restore/replacement.
Explicit folder opening canonicalizes off-thread and reuses an already-open
workspace. No JSON import runs: older JSON and project `.bed` files stay untouched.
`WorkspaceRoot` supplies browsing/cwd; only `FolderWorkspace` opts into persistence.
The default workspace keeps its settings, visibility and layouts in memory. Global
settings and explicitly saved global layout defaults persist normally.

Settings has no header or storage-location prose. Its sidebar contains:

- **General:** Open last workspace on open (Global only, default off), Autosave
  (Never, Change Focus, Timer), plus timeout for Timer.
- **Appearance:** Font family and size, Dark/Light theme, window opacity, macOS-only blur, panel opacity,
  border radius/gap, file-tree indentation and shader effects.
- **Editor:** Line numbers, smart wrapping and smart indentation, all enabled by default.

Startup policy is global because it is resolved before a workspace is opened.
With **Open last workspace on open** enabled, launching without a folder argument
reopens the last active folder workspace through normal session restoration,
including its layouts and active slot. An explicit folder argument takes precedence.
If the remembered folder is unavailable, startup reports the error and opens home.
The temporary home workspace does not replace the remembered folder.

Other values support workspace inheritance. Editing a Workspace value creates an
individual override; its reset icon resumes following Global. Unedited values
continue inheriting global changes. Switching workspaces applies effective
appearance to the window. Keybinding settings are deferred; Save, Open Folder,
Quit and layouts retain fixed Primary+S/O/Q and Primary+1–5 shortcuts. Primary is
Command on macOS and Control elsewhere. Widget-owned editing shortcuts retain
first refusal. The standalone Theme editor is retired.

**Font family** defaults to **Bedfont**, the bundled regular/bold Nordwand Mono
1.3.3 faces. The picker also lists installed system/user monospace families;
fixed-width primary faces preserve editor wrapping and terminal cell geometry.
**Font size** defaults to 15.5 points and accepts 8–72 points. Both values apply
to the UI, code editor and terminal, with independent Global/Workspace inheritance.
Changing fonts retains documents and running shells. A missing or unreadable
installed family falls back to Bedfont while retaining the saved family name.
Installed fonts are scanned once per application on a background worker started
with settings. Appearance and font inspection only read the current catalog;
while discovery is pending, Bedfont remains available and the picker shows a
loading message. Saved installed-family preferences apply after discovery finishes,
without permanently recording the interim fallback. Restart after installing a font.
A 2026-10-23 local probe measured 488 ms for 787 installed faces (six monospace
families); this scan previously blocked the first Appearance draw. Reproduce with
`cargo test --offline --locked -p bed2 system_font_discovery_latency_probe -- --ignored --nocapture`.

The RON settings fields are `font_family: "Bedfont"` and `font_size: 15.5`.
Older records acquire these defaults. Bundled Unicode fallbacks remain available,
and installed families use their closest available regular/bold weights. UI2 owns
the bundled bytes and [font provenance](../bedtime2/crates/bedtime-ui2/assets/fonts/UPSTREAM.md);
`fonts.rs` owns application family discovery, resolution and context replacement.
After building with `--features automation`, `python3 bedtime2/tools/smoke_fonts.py`
checks the real picker, live terminal resize, inheritance, fallback and persistence
using an isolated config root.

**Shader effects** enables whole-window CRT processing, disabled by default.
Independent 0–1 sliders control scanlines, vignette, bloom, static noise,
curvature, pixel grid, phosphor persistence and brightness pulse. Values remain
editable and persist while effects are off. Each value supports the same
Global/Workspace inheritance and reset behavior as other settings; older records
default to effects off. There are no presets, jitter or chromatic aberration.

`effects.rs` and its embedded `effects.wesl` own the application rendering.
Bevy runs persistence, when enabled, then CRT processing after egui and before
UI2's platform alpha conversion and final output. Effects operate on
display-premultiplied color and return linear-premultiplied color, preserving
transparent coverage and embedded viewport composition. Off bypasses all effect passes.
History textures belong to the camera's render entity, are allocated only for
persistence, and reset on resize, effect-value changes and workspace/layout
switches. Decay uses real elapsed frame time rather than frame count.

Bloom downsamples to half width/height, then uses a separable five-tap Gaussian
blur with three bilinear samples per axis. Two camera-owned `Rgba16Float`
textures hold display-premultiplied color, so blur samples require no color
conversion. The final CRT pass adds one filtered bloom sample at the curved
source coordinate. Bloom runs after persistence and before curvature; zero
intensity or disabled effects release its textures and skip all three bloom
passes. Resizing rounds reduced dimensions upward, including odd window sizes.
The blur approximates the original radial kernel with a similar four-pixel
radius; it preserves flat-color gain and source alpha, but is not pixel-identical.

Before UI2 draws, bed2 maps egui pointer movement, button and touch coordinates
through the same display-to-source curvature formula used by the shader. Native
cursor coordinates stay unchanged. Stationary pointers reproject when curvature
changes or is disabled. Input and rendering capture the same effective settings
before drawing, so Appearance edits take effect together on the next frame.
At the curved border, input follows the shader's clamped source coordinates.

The native regression uses an isolated config directory and checks curved clicks
and slider dragging, GPU color/alpha, resize/layout changes, viewport composition
and restoration after restart:

```sh
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_effects.py
```

The opt-in native GPU regression compares flat colors, text-like edges and
transparent regions against the original shader, and measures the complete
bloom + CRT GPU time at 4K using render-pass timestamps:

```sh
cargo test --offline --locked -p bed2 native_bloom_quality_and_gpu_cost -- --ignored --nocapture
```

It requires a timestamp-capable GPU; timing excludes image upload/readback.
On 2026-10-22, the Apple M4 Pro/Metal synthetic 4K comparison measured median
bloom + CRT GPU time of 2.900 ms for the original and 0.447 ms for the reduced
pipeline (6.49× faster). This measures the effect passes, not whole-app frame
time or thermal behavior. Frame pacing remains unchanged.

Document autosave is independent of metadata persistence. Timer defaults to one
second since the last content edit (supported timeout: 0.01–86400 seconds).
Change Focus saves when leaving an editor's text canvas, including panel/layout/
workspace changes and OS focus loss. Canonical document IDs deduplicate viewers;
inactive and viewless documents remain eligible. Composition, pending saves,
conflicts and missing bindings defer/prevent writes. Saves use the existing
source-checked service. Failures remain visible and do not retry the same revision
indefinitely; manual Save remains available. Quit decisions suspend new autosaves.

**View → Layout** selects layouts 1–5 and offers **Save Current Layout** and
**Reset Current Layout**. Switching retains panel/document/terminal instances.
A slot first uses its current global default, then retains its workspace edits.
Saving a default captures geometry, auxiliary tool placement, selection and focus,
without file paths, Code viewers or process state. It does not rearrange existing
workspace layouts. Reset reuses matching tools in stable tab order, creates
missing tools, and appends remaining panels to the largest area (first area wins
a size tie). Existing documents, drafts, histories and workers retain their owners.

The built-in layout 1 is one full-area Terminal. Only that layout is created at
startup; layouts 2–5 start empty when first selected. Saved global layout defaults
continue to apply to new layouts. Startup waits for
global preferences and workspace restoration before composing default tools, so
an initial shell cannot block restoration. Installing untouched defaults does not
create a workspace file.

Global and workspace stores validate bounded reads, merge untouched setting
values from late reads, buffer changes for 250 ms, and atomically replace files
on background workers. Workspace capture runs after layout edits and before
workspace retirement, and again after ordinary updates. Writes outlive retired
entities. Malformed/unsupported files and late composition conflicts are preserved
with visible errors; repair/access retry revalidates disk before saving live work.
Unresolved policy keeps editor/file operations read-only and blocks new terminals.
The quit barrier drains final snapshots before cancelling managed work.

Workspace records restore geometry, panel kinds, file/tree descriptors, tab
order/selection and area focus. They do not recover unsaved content/history/drafts,
editor cursor/folds/scroll, tree expansion/selection, terminal processes/transcripts
or viewport orbit state. Save As, New/Untitled creation, unsaved-content recovery
and broader workspace-management UI remain later work.

## Close decisions and application shutdown

Destructive panel/area UI intentions arrive as `PanelCloseRequested`. `close.rs`
currently approves through `commit_close` only while quit is idle and rejects
viewer-local text composition. `close_targets` preflights the exact views removed
by joins/docks using the same geometry planner as commit. Closing a view retains its canonical document and
edits; this is distinct from discarding a workspace. Workbench revalidates the
captured layout stamp at commit. Raw entity despawn bypasses decisions and prompts,
though ownership/worker cleanup still runs.

Closing a clean application flushes buffered metadata/settings and shuts down
without a save-workspace question. Only dirty/conflicted/composing documents need a
Save All / Discard / Cancel decision, across all workspaces including retained
viewless documents. Cancel retains live state and workers. Persistence failures
have a separate Retry / Cancel path, never a workspace Save/Discard decision.
Native actions obey the same close-modal gate; pending folder results resume on
cancellation, not after the final shutdown barrier. Already admitted editing
follow-ups are observed before deciding documents are clean. Quit preflights
closure, drains persistence, cancels managed work, waits for cleanup, then exits.

The quit policy retains Workbench's document gates for future integrations:
Save All awaits admitted writes, failures or changed revisions return to the
decision, and pending writes/drafts prevent destructive shutdown. File-backed Code documents participate in Save All, discard and cancel decisions;
missing-source documents require explicit discard or recovery.

## Verification

```sh
cargo test --offline --locked -p bed2 --features automation
cargo check --offline --locked -p bed2
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_bed2.py
python3 bedtime2/tools/smoke_cube.py
python3 bedtime2/tools/smoke_tasks.py
python3 bedtime2/tools/smoke_terminal.py
python3 bedtime2/tools/smoke_open.py
python3 bedtime2/tools/smoke_code.py
python3 bedtime2/tools/smoke_editor.py
python3 bedtime2/tools/smoke_filetree.py
python3 bedtime2/tools/smoke_filetree_drag.py
python3 bedtime2/tools/smoke_workspace_policy.py
```

The [automation protocol](../bedtime2/crates/bedtime-automation2/README.md) supports
stdin/stdout control, application input injection, runtime configuration, inspection
and rendered screenshots. Scenarios and assertions live outside `main.rs`.
The general scenario verifies sectioned settings, scope/reset behavior, layout
shortcuts, titlebar click/drag and rendered captures. The workspace-policy scenario
uses isolated config/home fixtures to verify real Timer/Change Focus autosaves,
Never/manual Save, temporary startup, centralized RON files and restart restoration. Generic snapshot/restore endpoints have been removed;
scenarios must use application-owned creation rather than `panel.open`. The other scenarios
inspect and exercise independent orbit viewports (`app.inspect_cube`), task and
process ownership/I/O (`app.inspect_tasks`), and real PTY sessions, input,
rendering, appearance and cleanup (`app.inspect_terminal`). Settings panels also
serve as cover tabs when checking hidden panels. Files/editor scenarios use temporary
projects to verify opening, previews, edits/saves, watchers, operations and cleanup.
The former Documents browser and its automation commands remain retired.
`app.inspect_menus` reports the shared tree and actual native installation;
`app.activate_menu` accepts a nested label path such as `["View", "Layout", "Layout 2"]` and revalidates the same action as a native
click. `app.open_folder` exercises the accepted directory-selection boundary, not
OS dialog clicking. `app.inspect_filetree` includes workspace roots, persistence
opt-in and settings/session readiness. `app.inspect_settings`/`app.set_settings`,
`app.inspect_layouts`/`app.select_layout` and `app.inspect_titlebar` expose the
application policy and actual titlebar geometry for automation. These are application operations, not a
reintroduced library panel registry.

Terminal support is initially macOS/Linux; native verification in this repository
has used macOS/Metal. The Windows task echo script and non-macOS native input
have not been verified here. Quit choices and worker-draining behavior have
application unit coverage; automation transport exit is separate from window quit.

Current workspace/menu policy verification is recorded in
[application composition](../MIGRATION/application-composition.md#follow-up-verification--2026-10-19):
the application and affected library suites passed, and the desktop policy scenario
verified native menus, actual home cwd, memory-only defaults, pre-quit autosave,
file/layout restore and clean quit. Files functional behavior passed; its full GPU
capture checks remain intermittent and unverified on the latest run. OS picker
clicking was not automated. Give each ordinary smoke a fresh `XDG_CONFIG_HOME` to
avoid modifying personal settings.

The following are historical results, predating the entity-first refactor; none
alone establishes verification of the current worktree.

Cleanup verified on 2026-10-18 on macOS/Metal: all five application quit tests,
normal bed2 compilation and the automation build passed. The four native scenarios
passed against the pinned `target/bed2-cleanup-smoke` executable: general workbench
(9 captures), viewport (8), tasks/processes (6) and terminal (15). Their summaries
and captures are under `target/bed2-cleanup-*-smoke` and
`target/bed2-cleanup-smoke-results`. Updated Python syntax and documentation links
also passed checks.


Tree-sitter/Structure verified on 2026-10-19: backend language/incremental tests,
editor feature suites, seven Structure lifecycle/navigation tests and the
application automation build passed. `smoke_code.py` passed eight native captures
for syntax, folding, Structure activation, shared views, placeholder, hidden-tab
reveal, automatic replacement focus and fold controls without line numbers.
Captures and summary are under
`target/bed2-code-smoke`. Its `app.inspect_code` command exposes current analysis,
view selections/folds and workspace source bindings without serializing buffers.

Files restoration verified on 2026-10-19 on macOS/Metal: 37 application tests,
100 filetree library tests, normal application checking and the automation build
passed. Native scenarios passed for file-backed syntax/Structure (8 captures,
`target/files-code-smoke`), editing/saving/shared views including a 112 MB document
(3 captures, `target/restored-editor-smoke`), and filetree operations, watching,
preview routing and persisted preferences (11 captures,
`target/restored-filetree-smoke`). Python syntax, Rust formatting and local
documentation links also passed.

Interactive LSP verification on 2026-10-22: 108 application, 57 editor, 29 LSP
and 27 UI tests passed; the real Rust/Go intelligence regression passed, including
standard-library hover/definitions. The native interaction smoke passed seven
nonempty captures and the existing LSP smoke passed five. The normal release
executable was rebuilt. Native Linux interaction verification remains pending.
