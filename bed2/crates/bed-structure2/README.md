# bEd code Structure panel

Application-owned code outline viewer over `bedtime-editor2` and
`bedtime-treesitter2`. Install `StructurePlugin` alongside `EditorAnalysisPlugin`.
`StructurePanel` is a concrete component with descriptive key `bed.structure`;
it creates no documents or editor panels. Its required `PanelInstance::of::<Self>()`
supplies metadata and drawing immediately, without registration or initialization.
Compose `(StructurePanel::default(), InArea(area))`; the spawn selects the tab in
that area without taking area focus. Applications own creation menus and any
session restore descriptors.
There is no arbitrary-document structure-provider protocol or application command
bus. The [integration plan](../../../MIGRATION/treesitter-and-structure-plan.md)
records the scope and package split.

The `StructureSource` world resource remembers one panel and its exact document
binding. After Workbench's focus and closure changes, current eligible editor
focus wins; otherwise a still-open remembered binding survives. Automatic tab
selection counts as ordinary focus. Closing the remembered source clears it
unless current focus supplies another eligible editor. There is no source history
or search through other viewers, including viewers of the same document.
Structure and Terminal focus preserve the remembered source. bEd's Files flow
explicitly sets that source when selecting or binding a file preview, so single
clicks update Structure while retaining Files focus.

Eligibility requires an open Workbench editor bound to a text document with a
supported `CodeLanguage`. During a same-source analysis refresh, each Structure
panel keeps its last outline, expansion and selection visible until the new
observation arrives. This includes the edit frame before analysis is resubmitted.
Cached rows never navigate with stale positions: activation still requires a
current observation. Source, session or language changes and analysis refusal
clear the cached outline. No available observation or cache displays
**No structure available**. A valid empty outline replaces the cache and displays
**No declarations**. A focused panel becoming eligible is reevaluated normally;
rebinding a remembered but unfocused panel cannot silently replace its document.
Tracking is world-local across layouts and retires with the session.

Each Structure panel owns its expansion and selection. Presentation positions include closing descendants during bed1-style 160 ms
smoothstep collapse; only viewport rows are drawn. Disclosure triangles match
Files and editor folds, and selected labels use bed1's RGB cycle. Reduced motion
settles expansion and uses a steady selection color.
Labels disclose their syntactic kind on hover. A captured `Activation` includes
the exact source binding, observation key and item ID. `activate` validates them
before using public editor navigation to select/reveal the name and focus the
existing source tab and layout. Composition and stale/closed/rebound sources
reject activation without changing layout, focus or selection. No panels are
reopened and no alternate viewer is chosen.

```sh
cargo test --offline --locked -p bed-structure2
cargo build --offline --locked -p bed2 --features automation
python3 bedtime2/tools/smoke_code.py
```

Historical verification before the entity-first refactor: on 2026-10-19 seven
package tests passed: remembered/current/automatic focus,
closure without fallback, rebinding and stale activation, shared analysis,
retained layout activation, composition rejection, and last-view retirement.
The application native smoke also passed eight captures covering syntax, folding,
Structure navigation, shared views, placeholder, hidden tabs, automatic focus and fold controls without
line numbers.
Files now opens file-backed Code documents through the same editor/analysis path,
including shared split views and detected/overridden languages. The Structure
contract is unchanged; persisted code sessions remain future application work.

Structure matches Files’ default 8-point left inset, 16-point indentation and
font size plus 10-point row height. Both use the same disclosure artwork and
item-sized selection backgrounds; Structure omits the file icon slot.
Its scroll viewport fills the panel, keeping horizontal scrollbars at the panel
bottom rather than immediately below short outlines. Bars appear only for overflow.
