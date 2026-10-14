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

#[test]
fn unused_branches_collapse_without_changing_live_panel_groups_or_split_sizes() {
    let ini = "[Window][bed_tab_1]\nDockId=0x4,2\n\n\
               [Window][bed_tab_2]\nDockId=0x6,0\n\n\
               [Window][bed_tab_3]\nDockId=0x6,1\n\n\
               [Docking][Data]\n\
               DockSpace ID=0x10 Window=0x20 Pos=0,0 Size=1200,800 Split=X\n\
               DockNode ID=0x1 Parent=0x10 SizeRef=240,800 Split=Y\n\
               DockNode ID=0x3 Parent=0x1 SizeRef=1400,600\n\
               DockNode ID=0x4 Parent=0x1 SizeRef=1400,200\n\
               DockNode ID=0x2 Parent=0x10 SizeRef=960,800 Split=X\n\
               DockNode ID=0x5 Parent=0x2 SizeRef=300,800\n\
               DockNode ID=0x6 Parent=0x2 SizeRef=660,800 CentralNode=1\n\
               DockNode ID=0x99 Pos=40,40 Size=500,500\n";
    let clean = prepare(ini, [1, 2, 3]).unwrap();
    assert!(clean.contains("DockId=0x4,2"));
    assert!(clean.contains("DockId=0x6,0"));
    assert!(clean.contains("DockId=0x6,1"));
    assert!(clean.contains("DockSpace ID=0x10 Window=0x20 Pos=0,0 Size=1200,800 Split=X"));
    assert!(clean.contains("DockNode ID=0x4 Parent=0x10 SizeRef=240,800"));
    assert!(clean.contains("DockNode ID=0x6 Parent=0x10 SizeRef=960,800 CentralNode=1"));
    assert_eq!(
        clean
            .lines()
            .filter(|line| line.starts_with("DockNode "))
            .count(),
        2
    );
    assert_eq!(prepare(&clean, [1, 2, 3]).unwrap(), clean);
}

#[test]
fn a_single_surviving_panel_moves_to_the_fixed_root_with_its_tab_order() {
    let ini = format!("[Window][bed_tab_1]\nDockId=0x2,3\n\n{TREE}");
    let clean = prepare(&ini, [1]).unwrap();
    assert!(clean.contains("DockId=0x10,3"));
    assert!(clean.contains("DockSpace ID=0x10 Window=0x20 Pos=0,0 Size=1200,800 CentralNode=1"));
    assert!(!clean.contains("DockNode "));
    assert_eq!(prepare(&clean, [1]).unwrap(), clean);
}

#[test]
fn an_empty_central_dock_stays_available_beside_saved_panels() {
    let ini = format!("[Window][bed_tab_1]\nDockId=0x1,0\n\n{TREE}");
    let clean = prepare(&ini, [1]).unwrap();
    assert!(clean.contains("DockNode ID=0x1 Parent=0x10 SizeRef=240,800"));
    assert!(clean.contains("DockNode ID=0x2 Parent=0x10 SizeRef=960,800 CentralNode=1"));
}
