//! Validate persisted docking data before passing it to ImGui's asserting parser.
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Node {
    children: usize,
    depth: usize,
    split: bool,
    central: bool,
    root: u32,
}

fn hex(value: &str) -> Result<u32, &'static str> {
    let value = value.strip_prefix("0x").ok_or("Invalid dock ID")?;
    if value.is_empty() || value.len() > 8 {
        return Err("Invalid dock ID");
    }
    u32::from_str_radix(value, 16).map_err(|_| "Invalid dock ID")
}

fn pair(value: &str, positive: bool) -> Result<(), &'static str> {
    let (x, y) = value.split_once(',').ok_or("Invalid dock dimensions")?;
    let x = x.parse::<i16>().map_err(|_| "Invalid dock dimensions")?;
    let y = y.parse::<i16>().map_err(|_| "Invalid dock dimensions")?;
    if positive && (x <= 0 || y <= 0) {
        return Err("Invalid dock dimensions");
    }
    Ok(())
}

/// Keep only this workspace's panel windows. Old releases accumulated windows
/// from previously opened workspaces, including references to nodes later split.
/// ImGui does not validate those references and aborts when updating such nodes.
pub(crate) fn prepare(
    ini: &str,
    panel_ids: impl IntoIterator<Item = u64>,
) -> Result<String, &'static str> {
    if ini.len() > 4 * 1024 * 1024 || ini.contains('\0') {
        return Err("Invalid workspace layout text");
    }
    let names: HashSet<_> = panel_ids
        .into_iter()
        .map(|id| format!("bed_tab_{id}"))
        .collect();
    let mut windows: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut dock_lines = Vec::new();
    let mut docking = false;
    let mut window = None;
    let mut seen_names = HashSet::new();
    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            docking = line == "[Docking][Data]";
            window = None;
            if let Some(name) = line
                .strip_prefix("[Window][")
                .and_then(|s| s.strip_suffix(']'))
            {
                let id = name.rsplit_once("###").map_or(name, |(_, id)| id);
                if names.contains(id) || name == "##bed_workspace" {
                    if !seen_names.insert(id) {
                        return Err("Duplicate panel window");
                    }
                    window = Some(windows.len());
                    windows.push((line, Vec::new()));
                }
            }
        } else if !line.is_empty() && !line.starts_with(';') {
            if docking {
                dock_lines.push(line);
            } else if let Some(index) = window {
                windows[index].1.push(line);
            }
        }
    }
    if dock_lines.is_empty() || dock_lines.len() > 4096 {
        return Err("Missing or oversized docking tree");
    }
    let mut nodes: HashMap<u32, Node> = HashMap::new();
    let mut central_roots = HashSet::new();
    let mut docking_text = String::from("[Docking][Data]\n");
    for line in dock_lines {
        let mut tokens = line.split_whitespace();
        let kind = tokens.next().ok_or("Invalid dock node")?;
        if !matches!(kind, "DockNode" | "DockSpace") {
            return Err("Invalid dock node");
        }
        let mut fields = HashMap::new();
        for token in tokens {
            let (key, value) = token.split_once('=').ok_or("Invalid dock node field")?;
            if fields.insert(key, value).is_some() {
                return Err("Duplicate dock node field");
            }
        }
        let id = hex(fields.get("ID").ok_or("Missing dock ID")?)?;
        if id == 0 || nodes.contains_key(&id) {
            return Err("Duplicate or zero dock ID");
        }
        let parent = fields.get("Parent").map(|value| hex(value)).transpose()?;
        if parent.is_some() && (fields.contains_key("Pos") || fields.contains_key("Size"))
            || parent.is_none() && fields.contains_key("SizeRef")
        {
            return Err("Unexpected dock dimensions");
        }
        let (root, depth) = if let Some(parent) = parent {
            if kind == "DockSpace" {
                return Err("Nested dock space");
            }
            // ImGui builds in file order. Requiring the parent first also rules
            // out cycles and self references before native code can recurse.
            let parent = nodes
                .get_mut(&parent)
                .ok_or("Missing or unordered dock parent")?;
            parent.children += 1;
            if parent.children > 2 {
                return Err("Dock node has more than two children");
            }
            if parent.depth >= 64 {
                return Err("Docking tree is too deep");
            }
            pair(
                fields.get("SizeRef").ok_or("Missing dock dimensions")?,
                true,
            )?;
            (parent.root, parent.depth + 1)
        } else {
            pair(fields.get("Pos").ok_or("Missing dock position")?, false)?;
            pair(fields.get("Size").ok_or("Missing dock dimensions")?, true)?;
            (id, 0)
        };
        if kind == "DockSpace" && !fields.contains_key("Window") {
            return Err("Missing dock space window");
        }
        if let Some(value) = fields.get("Window") {
            if hex(value)? == 0 || parent.is_some() {
                return Err("Invalid dock space window");
            }
        }
        if let Some(value) = fields.get("Selected") {
            hex(value)?;
        }
        let split = match fields.get("Split") {
            Some(&"X" | &"Y") => true,
            None => false,
            _ => return Err("Invalid dock split"),
        };
        let central = fields.get("CentralNode") == Some(&"1");
        if central && !central_roots.insert(root) {
            return Err("Multiple central dock nodes");
        }
        for key in [
            "NoResize",
            "CentralNode",
            "NoTabBar",
            "HiddenTabBar",
            "NoWindowMenuButton",
            "NoCloseButton",
        ] {
            if let Some(value) = fields.get(key) {
                if !matches!(*value, "0" | "1") {
                    return Err("Invalid dock flag");
                }
            }
        }
        nodes.insert(
            id,
            Node {
                root,
                depth,
                split,
                central,
                ..Node::default()
            },
        );
        // Emit the exact field order expected by ImGui's sscanf-based parser.
        docking_text.push_str(kind);
        for key in [
            "ID",
            "Parent",
            "Window",
            "Pos",
            "Size",
            "SizeRef",
            "Split",
            "NoResize",
            "CentralNode",
            "NoTabBar",
            "HiddenTabBar",
            "NoWindowMenuButton",
            "NoCloseButton",
            "Selected",
        ] {
            if let Some(value) = fields.get(key) {
                docking_text.push_str(&format!(" {key}={value}"));
            }
        }
        docking_text.push('\n');
    }
    for node in nodes.values() {
        if (node.split && node.children != 2)
            || (!node.split && node.children != 0)
            || (node.central && node.children != 0)
        {
            return Err("Invalid docking tree shape");
        }
    }
    let mut output = String::new();
    for (header, lines) in windows {
        output.push_str(header);
        output.push('\n');
        for line in lines {
            if let Some(value) = line.strip_prefix("DockId=") {
                let id = hex(value.split(',').next().unwrap_or(""))?;
                if id != 0 && !nodes.get(&id).is_some_and(|node| node.children == 0) {
                    return Err("Panel refers to a missing or split dock node");
                }
            }
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str(&docking_text);
    Ok(output)
}

#[cfg(test)]
mod tests {
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
        let mut deep =
            String::from("[Docking][Data]\nDockNode ID=0x1 Pos=0,0 Size=800,600 Split=X\n");
        for id in 2..=66 {
            deep.push_str(&format!(
                "DockNode ID=0x{id:X} Parent=0x{:X} SizeRef=800,600 Split=X\n",
                id - 1
            ));
        }
        assert_eq!(prepare(&deep, []), Err("Docking tree is too deep"));
    }
}
