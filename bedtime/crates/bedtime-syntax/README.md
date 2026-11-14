# bedtime-syntax

Tree-sitter analysis of source text, without Bevy or egui: highlights, folds,
an outline and embedded languages (a script in HTML, code in Markdown), all
at one revision.

```rust,ignore
use bedtime_syntax::{AnalysisSession, AnalysisStatus};
use std::sync::Arc;

let (mut session, worker) = AnalysisSession::with_worker();
std::thread::spawn(move || worker.run(|| false));
session.request(Arc::new(text), revision, "rust", None);
// Later, each frame:
session.poll();
if session.status() == &AnalysisStatus::Ready {
    let observation = session.observation().unwrap();
    for span in &observation.highlights { /* span.range, span.capture */ }
}
```

`AnalysisSession` parses on its worker, incrementally when every change since
the previous tree is known (pass that revision as `base`). Sources over 8 MiB
aren't analysed. An `Observation` holds the tree, sorted disjoint highlights
(`highlights_in` finds those in a range), folds, the outline and the
embedded languages (`language_at`).

`language_name` finds a language by file name, `LANGUAGES` lists them, and
`comment_tokens` gives each one's comments. The grammars are
`bedtime-grammars`; tree-sitter is re-exported as `tree_sitter` and pinned here
once (Cargo allows only one `links = "tree-sitter"`).

How highlights are coloured is the theme's: `bedtime_ui::theme::SyntaxPalette`.
bedtime-code runs this for the documents its editors show (`DocumentSyntax`).
