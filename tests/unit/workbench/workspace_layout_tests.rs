use super::*;
const TREE: &str = "[Docking][Data]\nDockSpace ID=0x10 Window=0x20 Pos=0,0 Size=1200,800 Split=X\nDockNode ID=0x1 Parent=0x10 SizeRef=240,800\nDockNode ID=0x2 Parent=0x10 SizeRef=960,800 CentralNode=1\n";

#[test]
fn obsolete_windows_are_removed_and_live_split_references_are_rejected() {
    let ini = format!(
        "[Window][bed_tab_1]\nDockId=0x10,0\n\n[Window][bed_tab_2]\nDockId=0x2,0\n\n{TREE}"
    );
    let clean = prepare(&ini, [2]).unwrap();
    assert!(!clean.contains("[Window][bed_tab_1]"));
    assert!(clean.contains("[Window][bed_tab_2]"));
    assert!(prepare(&ini, [1, 2]).is_err());
    assert!(prepare(&ini.replace("DockId=0x2", "DockId=0x99"), [2]).is_err());
}

#[test]
fn malformed_trees_are_rejected_before_native_parsing() {
    for invalid in [
        TREE.replace("ID=0x2", "ID=0x1"),
        TREE.replace("Parent=0x10", "Parent=0x2"),
        TREE.replace("Split=X", "Split=Z"),
        TREE.replace("SizeRef=240,800", "SizeRef=0,800"),
        TREE.replace("Size=1200,800", "Size=40000,800"),
        TREE.replace("SizeRef=240,800", "SizeRef=240,800 CentralNode=1"),
        TREE.replace(" Split=X", ""),
        format!("{TREE}DockNode ID=0x3 Parent=0x10 SizeRef=20,20\n"),
        TREE.lines().take(3).collect::<Vec<_>>().join("\n"),
        TREE.replace("Parent=0x10", "Parent=0x1"),
    ] {
        assert!(prepare(&invalid, []).is_err(), "{invalid}");
    }
    let mut deep = String::from("[Docking][Data]\nDockNode ID=0x1 Pos=0,0 Size=800,600 Split=X\n");
    for id in 2..=66 {
        deep.push_str(&format!(
            "DockNode ID=0x{id:X} Parent=0x{:X} SizeRef=800,600 Split=X\n",
            id - 1
        ));
    }
    assert_eq!(prepare(&deep, []), Err("Docking tree is too deep"));
}
