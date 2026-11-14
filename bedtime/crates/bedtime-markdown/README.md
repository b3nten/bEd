# bedtime-markdown

Workbench Markdown viewer: the document rendered. Parsing and drawing are
`bedtime_ui::markdown` (`MarkdownView`, also used for language server
tooltips); this crate shows a text document with it, colours its code from
the document's analysis, and follows its links.

```rust,ignore
app.add_plugins(MarkdownPlugin);
```

- **The document** is text (`TextBuffer`), the one a text editor shows, so
  the rendering follows the editor's edits, unsaved too.
- **Not the default.** Markdown files open in the editor; this viewer is in
  "Open With", and it's an editor's `other_viewer`. An app can make it the
  default with
  `set_default_viewer::<MarkdownPanel>(MARKDOWN)`.
- **Code blocks** are coloured by the document's syntax analysis, shared
  with its editors. After an edit the view waits briefly for the colours
  instead of flashing uncoloured code.
- **Links** open in the browser, relative files in the app, and `#anchors`
  scroll to their heading. **Images** beside the file (or at absolute paths)
  show in place, at their own size or the text's width.

```sh
cargo test -p bedtime-markdown
```
