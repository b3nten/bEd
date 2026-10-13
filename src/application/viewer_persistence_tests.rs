use super::*;

#[test]
fn different_viewer_tabs_restore_separately_with_shared_document_and_per_tab_state() {
    let dir = TempDir::new();
    let mut original = app(&dir);
    let path = dir.write("values.json", b"{\"value\":1}");
    let text = open_file(
        original.world_mut(),
        &path,
        Some(bed_plugin_editor::TEXT_VIEWER),
        false,
        None,
    )
    .unwrap();
    let view = original
        .world()
        .get::<PanelBinding>(text)
        .unwrap()
        .view
        .unwrap();
    original
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(
                0,
                5,
                false,
                bed_editing::text_commands::CursorReveal::Ensure,
            );
        })
        .unwrap();
    let json = open_viewer(original.world_mut(), text, "bed.json.viewer").unwrap();
    assert_ne!(text, json);
    let saved = layouts::snapshot(original.world_mut()).unwrap()["layouts"]
        [original.world().resource::<LayoutSlots>().active().key()]
    .clone();
    let viewer_records = saved["panels"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| {
            record["viewer"] == bed_plugin_editor::TEXT_VIEWER
                || record["viewer"] == "bed.json.viewer"
        })
        .collect::<Vec<_>>();
    assert_eq!(viewer_records.len(), 2);
    assert!(
        viewer_records
            .iter()
            .all(|record| record.get("presentations").is_none())
    );
    let text_record = viewer_records
        .iter()
        .find(|record| record["viewer"] == bed_plugin_editor::TEXT_VIEWER)
        .unwrap();
    assert_eq!(text_record["state"]["selections"][0][1], 5);

    let fresh = TempDir::new();
    let mut restored = app(&fresh);
    restore_layout(restored.world_mut(), &saved, false).unwrap();
    let tabs = panel_entities(restored.world_mut())
        .into_iter()
        .filter(|panel| {
            restored
                .world()
                .get::<PanelBinding>(*panel)
                .is_some_and(|binding| {
                    binding.viewer.as_deref() == Some(bed_plugin_editor::TEXT_VIEWER)
                        || binding.viewer.as_deref() == Some("bed.json.viewer")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(tabs.len(), 2);
    let bindings = tabs
        .iter()
        .map(|tab| restored.world().get::<PanelBinding>(*tab).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bindings[0].document, bindings[1].document);
    let text = bindings
        .iter()
        .find(|binding| binding.viewer.as_deref() == Some(bed_plugin_editor::TEXT_VIEWER))
        .unwrap();
    let snapshot = restored
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(text.view.unwrap())
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (0, 5));
    let resaved = layouts::snapshot(restored.world_mut()).unwrap()["layouts"]
        [restored.world().resource::<LayoutSlots>().active().key()]
    .clone();
    assert_eq!(
        resaved["panels"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|record| record["viewer"] == bed_plugin_editor::TEXT_VIEWER
                || record["viewer"] == "bed.json.viewer")
            .count(),
        2
    );
}
