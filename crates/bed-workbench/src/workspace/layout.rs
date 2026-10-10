//! Validate and compact legacy docking data before migrating it to tiled areas.
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Node<'a> {
    children: Vec<u32>,
    parent: Option<u32>,
    depth: usize,
    split: bool,
    central: bool,
    root: u32,
    kind: &'a str,
    fields: HashMap<&'a str, &'a str>,
}

/// Drop unused leaves and lift a sole surviving child into its parent's space.
/// Dock-space IDs stay fixed; window references follow a leaf lifted to a root.
fn prune_empty_branches(
    id: u32,
    nodes: &mut HashMap<u32, Node<'_>>,
    occupied: &HashSet<u32>,
    remapped: &mut HashMap<u32, u32>,
) -> Option<u32> {
    let mut node = nodes.remove(&id).unwrap();
    node.children = node
        .children
        .into_iter()
        .filter_map(|child| prune_empty_branches(child, nodes, occupied, remapped))
        .collect();
    match node.children.len() {
        0 => {
            if !occupied.contains(&id) && !node.central && node.kind != "DockSpace" {
                return None;
            }
            node.fields.remove("Split");
        }
        1 => {
            let child_id = node.children[0];
            let mut child = nodes.remove(&child_id).unwrap();
            let replacement = if node.parent.is_none() { id } else { child_id };
            // Keep the space allocated to this branch, rather than the child's
            // stale dimensions from before its sibling disappeared.
            for key in ["Window", "Pos", "Size", "SizeRef"] {
                if let Some(value) = node.fields.get(key) {
                    child.fields.insert(key, value);
                } else {
                    child.fields.remove(key);
                }
            }
            if replacement != child_id {
                child.fields.insert("ID", node.fields["ID"]);
                remapped.insert(child_id, replacement);
            }
            child.kind = node.kind;
            child.parent = node.parent;
            for descendant in &child.children {
                nodes.get_mut(descendant).unwrap().parent = Some(replacement);
            }
            nodes.insert(replacement, child);
            return Some(replacement);
        }
        2 => {
            for child in &node.children {
                nodes.get_mut(child).unwrap().parent = Some(id);
            }
        }
        _ => unreachable!("validated dock nodes have at most two children"),
    }
    nodes.insert(id, node);
    Some(id)
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

/// Give restored workspace panels fresh IDs after compacting its dock tree.
/// Window, viewport, and selected-tab references follow the renamed panels.
#[cfg(test)]
pub(crate) fn remap_panel_ids(ini: &str, ids: &HashMap<u64, u64>) -> Result<String, &'static str> {
    if ids.values().collect::<HashSet<_>>().len() != ids.len() {
        return Err("Duplicate remapped panel ID");
    }
    let prepared = prepare(ini, ids.keys().copied())?;
    let mut windows = HashMap::new();
    let mut tabs = HashMap::new();
    for (&old, &new) in ids {
        let old_window = hash(&format!("bed_tab_{old}"), 0);
        let new_window = hash(&format!("bed_tab_{new}"), 0);
        windows.insert(old_window, new_window);
        // Docking persists ImGuiWindow::TabId, which is GetID("#TAB")
        // inside the window's ID scope rather than the window ID itself.
        tabs.insert(hash("#TAB", old_window), hash("#TAB", new_window));
    }
    let mut output = String::new();
    let mut docking = false;
    for line in prepared.lines() {
        if let Some(name) = line
            .strip_prefix("[Window][")
            .and_then(|name| name.strip_suffix(']'))
        {
            docking = false;
            let (prefix, id) = name
                .rsplit_once("###")
                .map_or((None, name), |(prefix, id)| (Some(prefix), id));
            let replacement = id
                .strip_prefix("bed_tab_")
                .and_then(|id| id.parse::<u64>().ok())
                .and_then(|id| ids.get(&id));
            if let Some(id) = replacement {
                output.push_str("[Window][");
                if let Some(prefix) = prefix {
                    output.push_str(prefix);
                    output.push_str("###");
                }
                output.push_str(&format!("bed_tab_{id}]\n"));
                continue;
            }
        } else if line.starts_with('[') {
            docking = line == "[Docking][Data]";
        }
        if docking && !line.starts_with('[') {
            let fields: Result<Vec<_>, _> = line
                .split_whitespace()
                .map(|field| {
                    if let Some(value) = field.strip_prefix("Selected=") {
                        remap_reference("Selected", value, &tabs)
                    } else if let Some(value) = field.strip_prefix("Window=") {
                        remap_reference("Window", value, &windows)
                    } else {
                        Ok(field.to_owned())
                    }
                })
                .collect();
            output.push_str(&fields?.join(" "));
        } else if let Some(value) = line.strip_prefix("ViewportId=") {
            output.push_str(&remap_reference("ViewportId", value, &windows)?);
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    prepare(&output, ids.values().copied())
}

#[cfg(test)]
fn hash(value: &str, seed: u32) -> u32 {
    // ImHashStr only reads the supplied bytes and its static CRC lookup table;
    // it neither accesses nor requires a live ImGui context.
    unsafe { dear_imgui_rs::sys::igImHashStr(value.as_ptr().cast(), value.len(), seed) }
}

#[cfg(test)]
fn remap_reference(
    key: &str,
    value: &str,
    ids: &HashMap<u32, u32>,
) -> Result<String, &'static str> {
    let old = hex(value)?;
    Ok(match ids.get(&old) {
        Some(new) => format!("{key}=0x{new:08X}"),
        None => format!("{key}={value}"),
    })
}

/// Keep this workspace's panel windows and remove unused docking branches.
/// Validate references before pruning: ImGui's parser asserts on invalid trees.
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
    let mut nodes: HashMap<u32, Node<'_>> = HashMap::new();
    let mut roots = Vec::new();
    let mut central_roots = HashSet::new();
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
            parent.children.push(id);
            if parent.children.len() > 2 {
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
            roots.push(id);
            pair(fields.get("Pos").ok_or("Missing dock position")?, false)?;
            pair(fields.get("Size").ok_or("Missing dock dimensions")?, true)?;
            (id, 0)
        };
        if kind == "DockSpace" && !fields.contains_key("Window") {
            return Err("Missing dock space window");
        }
        if let Some(value) = fields.get("Window")
            && (hex(value)? == 0 || parent.is_some())
        {
            return Err("Invalid dock space window");
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
            if let Some(value) = fields.get(key)
                && !matches!(*value, "0" | "1")
            {
                return Err("Invalid dock flag");
            }
        }
        nodes.insert(
            id,
            Node {
                root,
                depth,
                split,
                central,
                parent,
                kind,
                fields,
                ..Node::default()
            },
        );
    }
    for node in nodes.values() {
        if (node.split && node.children.len() != 2)
            || (!node.split && !node.children.is_empty())
            || (node.central && !node.children.is_empty())
        {
            return Err("Invalid docking tree shape");
        }
    }
    let mut occupied = HashSet::new();
    for (_, lines) in &windows {
        for line in lines {
            if let Some(value) = line.strip_prefix("DockId=") {
                let id = hex(value.split(',').next().unwrap_or(""))?;
                if id != 0 {
                    if !nodes.get(&id).is_some_and(|node| node.children.is_empty()) {
                        return Err("Panel refers to a missing or split dock node");
                    }
                    occupied.insert(id);
                }
            }
        }
    }
    let mut remapped = HashMap::new();
    roots.retain(|&id| prune_empty_branches(id, &mut nodes, &occupied, &mut remapped).is_some());
    let mut output = String::new();
    for (header, lines) in windows {
        output.push_str(header);
        output.push('\n');
        for line in lines {
            if let Some(value) = line.strip_prefix("DockId=") {
                let id = hex(value.split(',').next().unwrap_or(""))?;
                if let Some(new) = remapped.get(&id) {
                    output.push_str(&format!("DockId=0x{new:X}"));
                    if let Some((_, order)) = value.split_once(',') {
                        output.push(',');
                        output.push_str(order);
                    }
                    output.push('\n');
                    continue;
                }
            }
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str("[Docking][Data]\n");
    let mut pending = roots.into_iter().rev().collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        let node = &nodes[&id];
        output.push_str(node.kind);
        // ImGui's sscanf-based parser requires this exact field order.
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
            if key == "Parent" {
                if let Some(parent) = node.parent {
                    output.push_str(&format!(" Parent=0x{parent:X}"));
                }
            } else if let Some(value) = node.fields.get(key) {
                output.push_str(&format!(" {key}={value}"));
            }
        }
        output.push('\n');
        pending.extend(node.children.iter().rev().copied());
    }
    Ok(output)
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/workspace_layout_tests.rs"]
mod tests;

#[cfg(test)]
mod remap_tests {
    use super::*;

    #[test]
    fn remaps_panel_names_selection_and_viewports_after_pruning_unused_docks() {
        let old_window = hash("bed_tab_1", 0);
        let old_tab = hash("#TAB", old_window);
        let new_window = hash("bed_tab_11", 0);
        let new_tab = hash("#TAB", new_window);
        let ini = format!(
            "[Window][shell###bed_tab_1]\nViewportId=0x{old_window:08X}\nDockId=0x2,0\n\n\
             [Window][bed_tab_2]\nDockId=0x2,1\n\n\
             [Window][obsolete###bed_tab_3]\nDockId=0x99\n\n\
             [Docking][Data]\n\
             DockSpace ID=0x10 Window=0x{old_window:08X} Pos=0,0 Size=1200,800 Split=X\n\
             DockNode ID=0x1 Parent=0x10 SizeRef=240,800\n\
             DockNode ID=0x2 Parent=0x10 SizeRef=960,800 CentralNode=1 Selected=0x{old_tab:08X}\n"
        );
        let remapped = remap_panel_ids(&ini, &HashMap::from([(1, 11), (2, 12)])).unwrap();
        assert!(remapped.contains("[Window][shell###bed_tab_11]"));
        assert!(remapped.contains("[Window][bed_tab_12]"));
        assert!(!remapped.contains("obsolete"));
        assert!(remapped.contains(&format!("ViewportId=0x{new_window:08X}")));
        assert!(remapped.contains(&format!("Window=0x{new_window:08X}")));
        assert!(remapped.contains(&format!("Selected=0x{new_tab:08X}")));
        assert!(remapped.contains("DockId=0x10,0"));
        assert!(remapped.contains("DockId=0x10,1"));
        assert!(!remapped.contains("DockNode "));
        assert_eq!(prepare(&remapped, [11, 12]).unwrap(), remapped);
    }

    #[test]
    fn remapping_rejects_collisions_and_invalid_layouts() {
        let ini = "[Window][bed_tab_1]\nDockId=0x1\n\n[Docking][Data]\nDockSpace ID=0x1 Window=0x20 Pos=0,0 Size=800,600 CentralNode=1\n";
        assert_eq!(
            remap_panel_ids(ini, &HashMap::from([(1, 11), (2, 11)])),
            Err("Duplicate remapped panel ID")
        );
        assert!(
            remap_panel_ids(
                &ini.replace("DockId=0x1", "DockId=0x99"),
                &HashMap::from([(1, 11)])
            )
            .is_err()
        );
        let remapped = remap_panel_ids(ini, &HashMap::from([(1, 11)])).unwrap();
        assert!(
            remapped.contains("Window=0x20"),
            "unrelated host IDs stay unchanged"
        );
    }
}
