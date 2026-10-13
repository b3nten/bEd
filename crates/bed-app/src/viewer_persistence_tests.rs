use super::*;

#[test]
fn resaving_an_active_viewer_keeps_unmounted_viewer_state_and_opaque_extensions() {
    let dir = TempDir::new();
    let mut original = app(&dir);
    let path = dir.write("values.json", b"{\"value\":1}");
    let canonical = std::fs::canonicalize(&path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let panel = open_file(
        original.world_mut(),
        &path,
        Some(bed_plugin_editor::TEXT_VIEWER),
        false,
        None,
    )
    .unwrap();
    let view = original
        .world()
        .get::<PanelBinding>(panel)
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
    switch_viewer(original.world_mut(), panel, "bed.json.viewer").unwrap();
    persist_workspace(original.world_mut()).unwrap();
    let mut saved = original
        .world()
        .non_send::<WorkspacePersistence>()
        .store
        .standalone_layout()
        .unwrap()
        .clone();
    let record = saved["panels"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["path"] == canonical.as_str())
        .unwrap();
    let text_state = record["presentations"][bed_plugin_editor::TEXT_VIEWER].clone();
    assert_eq!(text_state["selections"][0][1], 5);
    let opaque = json!({"future_revision":3,"selection":{"node":"keep"}});
    record["presentations"]["third.party.viewer"] = opaque.clone();

    let fresh = TempDir::new();
    let mut restored = app(&fresh);
    restore_layout(restored.world_mut(), &saved, false).unwrap();
    // Only the active JSON presentation has been mounted in this new app.
    // Capturing it must merge with inactive serialized presentations.
    persist_workspace(restored.world_mut()).unwrap();
    let resaved = restored
        .world()
        .non_send::<WorkspacePersistence>()
        .store
        .standalone_layout()
        .unwrap()
        .clone();
    let record = resaved["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["path"] == canonical.as_str())
        .unwrap();
    assert_eq!(record["viewer"], "bed.json.viewer");
    assert_eq!(
        record["presentations"][bed_plugin_editor::TEXT_VIEWER],
        text_state
    );
    assert_eq!(record["presentations"]["third.party.viewer"], opaque);

    let another = TempDir::new();
    let mut again = app(&another);
    restore_layout(again.world_mut(), &resaved, false).unwrap();
    let panel = panel_entities(again.world_mut())
        .into_iter()
        .find(|panel| {
            again
                .world()
                .get::<PanelBinding>(*panel)
                .and_then(|binding| binding.document)
                .is_some_and(|document| {
                    again
                        .world()
                        .non_send::<DocumentSession>()
                        .snapshot(document)
                        .unwrap()
                        .path
                        == canonical
                })
        })
        .unwrap();
    switch_viewer(again.world_mut(), panel, bed_plugin_editor::TEXT_VIEWER).unwrap();
    let view = again
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    let restored_view = again
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!((restored_view.row, restored_view.column), (0, 5));
}
