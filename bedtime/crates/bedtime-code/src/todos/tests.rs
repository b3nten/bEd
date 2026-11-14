use super::{scan::*, *};

fn item(relative: &str, line: u32, tag: &'static str, text: &str) -> TodoItem {
    TodoItem {
        path: PathBuf::from("/w").join(relative),
        relative: relative.into(),
        found: Found {
            line,
            column: 0,
            tag,
            author: None,
            text: text.into(),
        },
    }
}

fn sample() -> Scan {
    Scan {
        root: "/w".into(),
        items: vec![
            item("README", 0, "TODO", "write it"),
            item("src/a/b/deep.rs", 3, "HACK", "zebra"),
            item("src/lib.rs", 1, "TODO", "apple"),
            item("src/lib.rs", 9, "FIXME", "mango"),
        ],
        ..default()
    }
}

#[test]
fn finds_tags_in_comments_only() {
    let text = "\
fn main() {
    // TODO: tidy this up
    let todo = \"TODO not a comment\";
    /* FIXME(ben): leaks */
     * XXX in a block
    # HACK - shell style
    -- BUG sql style
    <!-- TODO html -->
    // TODOs isn't a tag, nor is NOTTODO
    // todo lowercase isn't either
}";
    let found = find(text);
    let summary: Vec<_> = found
        .iter()
        .map(|f| (f.line, f.tag, f.text.as_str(), f.author.as_deref()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (1, "TODO", "tidy this up", None),
            (3, "FIXME", "leaks", Some("ben")),
            (4, "XXX", "in a block", None),
            (5, "HACK", "shell style", None),
            (6, "BUG", "sql style", None),
            (7, "TODO", "html", None),
        ]
    );
    // Columns count characters, for selecting the tag.
    assert_eq!(found[0].column, 7);
    assert_eq!(find("// é TODO x")[0].column, 5);
}

#[test]
fn scans_a_folder_respecting_ignore_files() {
    let root = std::env::temp_dir().join(format!("bed-todos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "// TODO second\n\n// FIXME third\n").unwrap();
    std::fs::write(root.join("a.py"), "# TODO first\n").unwrap();
    std::fs::write(root.join("target/out.rs"), "// TODO ignored\n").unwrap();
    std::fs::write(root.join("bin"), b"\0// TODO binary").unwrap();
    let scan = scan(&root, &|| false);
    let found: Vec<_> = scan
        .items
        .iter()
        .map(|i| (i.relative.as_str(), i.found.line, i.found.text.as_str()))
        .collect();
    assert_eq!(
        found,
        vec![
            ("a.py", 0, "first"),
            ("src/b.rs", 0, "second"),
            ("src/b.rs", 2, "third"),
        ]
    );
    assert!(!scan.limited && !scan.cancelled);
    assert!(super::scan::scan(&root, &|| true).cancelled);
    let _ = std::fs::remove_dir_all(&root);
}

fn items(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Item { index, .. } => Some(*index),
            _ => None,
        })
        .collect()
}

#[test]
fn the_list_sorts_by_file_tag_or_text() {
    let scan = sample();
    let mut panel = TodoPanel {
        layout: TodoLayout::List,
        ..default()
    };
    assert_eq!(items(&arrange(&panel, &scan, "")), vec![0, 1, 2, 3]);
    panel.sort = TodoSort::Tag;
    assert_eq!(items(&arrange(&panel, &scan, "")), vec![3, 0, 2, 1]);
    panel.sort = TodoSort::Text;
    assert_eq!(items(&arrange(&panel, &scan, "")), vec![2, 3, 0, 1]);
    panel.descending = true;
    assert_eq!(items(&arrange(&panel, &scan, "")), vec![1, 0, 3, 2]);
}

#[test]
fn filtering_and_hidden_tags() {
    let scan = sample();
    let mut panel = TodoPanel {
        layout: TodoLayout::List,
        ..default()
    };
    // Text, tag and path all count.
    assert_eq!(items(&arrange(&panel, &scan, "MANGO")), vec![3]);
    assert_eq!(items(&arrange(&panel, &scan, "hack")), vec![1]);
    assert_eq!(items(&arrange(&panel, &scan, "lib.rs")), vec![2, 3]);
    panel.hidden_tags = vec!["TODO".into()];
    assert_eq!(items(&arrange(&panel, &scan, "")), vec![1, 3]);
}

#[test]
fn the_tree_groups_by_folder_and_file() {
    let scan = sample();
    let panel = TodoPanel::default();
    let rows = arrange(&panel, &scan, "");
    let shape: Vec<String> = rows
        .iter()
        .map(|row| match row {
            Row::Folder { label, depth, .. } => format!("{depth} {label}/"),
            Row::File {
                label,
                depth,
                count,
                ..
            } => format!("{depth} {label} ({count})"),
            Row::Item { index, depth } => format!("{depth} #{index}"),
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            // Folders first; a folder with only a folder in it is one row.
            "0 src/",
            "1 a/b/",
            "2 deep.rs (1)",
            "3 #1",
            "1 lib.rs (2)",
            "2 #2",
            "2 #3",
            "0 README (1)",
            "1 #0",
        ]
    );

    // Collapsing hides what's inside.
    let collapsed: HashSet<String> = ["src/a/b".to_owned(), "README".to_owned()].into();
    let rows = visible(rows, &collapsed);
    assert_eq!(items(&rows), vec![2, 3]);
    assert_eq!(rows.len(), 6);
}
