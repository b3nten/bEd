// Shared text and byte panel ownership, factories, and persisted view state.
#[derive(Component, Clone, Copy, Debug)]
pub struct EditorPanelState {
    pub document: DocumentId,
    pub initial_revision: Option<bed_api::documents::Revision>,
    pub hex: bool,
}

pub(crate) enum Widget {
    Text(EditorView),
    Hex(RefCell<HexEditor>),
}

#[derive(Default)]
pub struct EditorViews {
    pub(crate) widgets: HashMap<Entity, Widget>,
}

impl EditorViews {
    pub fn text(&self, entity: Entity) -> Option<&EditorView> {
        match self.widgets.get(&entity)? {
            Widget::Text(view) => Some(view),
            Widget::Hex(_) => None,
        }
    }

    pub fn text_mut(&mut self, entity: Entity) -> Option<&mut EditorView> {
        match self.widgets.get_mut(&entity)? {
            Widget::Text(view) => Some(view),
            Widget::Hex(_) => None,
        }
    }

    pub fn hex(&self, entity: Entity) -> Option<std::cell::Ref<'_, HexEditor>> {
        match self.widgets.get(&entity)? {
            Widget::Hex(editor) => Some(editor.borrow()),
            Widget::Text(_) => None,
        }
    }
}

#[derive(Resource)]
struct Systems {
    actions: bed_api::workbench::PanelActionSystem,
    clipboard: bed_api::workbench::PanelClipboardSystem,
    focus: bed_api::workbench::PanelLifecycleSystem,
    close: bed_api::workbench::PanelLifecycleSystem,
}

/// Attach a text presentation owned by another feature, such as a Git diff.
/// It shares the document session, action systems, and close lifecycle.
pub fn create_embedded(
    world: &mut World,
    entity: Entity,
    document: DocumentId,
    state: &Value,
) -> Result<bed_documents::ViewId, String> {
    let mut binding = world
        .get::<PanelBinding>(entity)
        .cloned()
        .unwrap_or_default();
    binding.document = Some(document);
    binding.input = bed_api::documents::PanelInput::Document(document);
    world.entity_mut(entity).insert(binding);
    create_editor(
        world,
        PanelCreation {
            entity,
            state: state.clone(),
        },
        false,
    )?;
    Ok(world
        .get::<PanelBinding>(entity)
        .expect("an embedded editor has a binding")
        .view
        .expect("a text presentation has a managed view"))
}

fn create_hex(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_editor(world, creation, true)
}

fn create_editor(world: &mut World, creation: PanelCreation, hex: bool) -> PanelResult {
    let mut notes = if hex {
        bed_api::workbench::PanelDocumentation::new(&[
            ("Bytes", "Inspect and edit bytes in hexadecimal or text. Select a range to copy it; Toggle Insert Mode switches between inserting and replacing bytes."),
            ("Views", "The viewer menu switches between text and hex presentations of the same document."),
        ]).shortcut_actions(&[
            "bed.editor.delete_left", "bed.editor.delete_right",
            "bed.navigation.move_left", "bed.navigation.move_right", "bed.navigation.move_up", "bed.navigation.move_down",
            "bed.editor.move_line_start", "bed.editor.move_line_end", "bed.editor.move_doc_start", "bed.editor.move_doc_end",
            "bed.navigation.select_left", "bed.navigation.select_right", "bed.navigation.select_up", "bed.navigation.select_down",
            "bed.editor.select_line_start", "bed.editor.select_line_end", "bed.editor.select_doc_start", "bed.editor.select_doc_end",
        ])
    } else if cfg!(feature = "language-services") {
        bed_api::workbench::PanelDocumentation::new(&[
            (
                "Edit",
                "Type to edit the document. Use Find or Go to Line to navigate, and the gutter to fold code or set breakpoints.",
            ),
            (
                "Code",
                "Go to Definition and Find References use the language server. Add cursors above or below to edit multiple lines.",
            ),
            (
                "View",
                "Toggle word wrap or the minimap from the editor menu. Zoom changes this view’s text size; other views keep their own zoom.",
            ),
        ])
    } else {
        bed_api::workbench::PanelDocumentation::new(&[
            (
                "Edit",
                "Type to edit the document. Find and Go to Line navigate within it. Add cursors above or below to edit multiple lines.",
            ),
            (
                "View",
                "Word wrap, the minimap and text zoom belong to this view. Other views share the document and its undo history.",
            ),
        ])
    };
    notes.shortcut_actions.extend(
        [
            "bed.edit.copy",
            "bed.edit.cut",
            "bed.edit.paste",
            "bed.edit.select_all",
            "bed.edit.undo",
            "bed.edit.redo",
        ]
        .map(str::to_owned),
    );
    if !hex {
        notes.shortcut_actions.extend(
            [
                "bed.edit.find",
                bed_keybindings::ZOOM_IN,
                bed_keybindings::ZOOM_OUT,
                bed_keybindings::RESET_ZOOM,
            ]
            .into_iter()
            .chain([
                "bed.navigation.move_left",
                "bed.navigation.move_right",
                "bed.navigation.move_up",
                "bed.navigation.move_down",
                "bed.navigation.select_left",
                "bed.navigation.select_right",
                "bed.navigation.select_up",
                "bed.navigation.select_down",
            ])
            .map(str::to_owned),
        );
    }
    world.entity_mut(creation.entity).insert(notes);
    let mut binding = world
        .get::<PanelBinding>(creation.entity)
        .cloned()
        .unwrap_or_default();
    if binding.input.local_file().is_some() {
        return Err("Editor requires a document, not a local-file panel".into());
    }
    let mut session = world
        .get_non_send_mut::<DocumentSession>()
        .ok_or("Editor document session is unavailable")?;
    let created = binding.input.document().or(binding.document).is_none();
    let document = match binding.input.document().or(binding.document) {
        Some(document) => document,
        None => session
            .create_document_with_kind(
                &[],
                if hex {
                    DocumentKind::Bytes
                } else {
                    DocumentKind::Text
                },
            )
            .map_err(|error| error.to_string())?,
    };
    let initial_revision = pristine_revision(&session, document);
    let result = (|| -> io::Result<Widget> {
        if hex {
            if session.document_kind(document)? != DocumentKind::Bytes {
                return Err(io::Error::other(
                    "Hex editor requires an exact-byte document",
                ));
            }
            let mut editor = HexEditor::new(document);
            editor.restore_state(&creation.state);
            editor.synchronize(&session)?;
            Ok(Widget::Hex(RefCell::new(editor)))
        } else {
            let view = if let Some(view) = binding.view {
                if session.document_for_view(view) != Some(document) {
                    return Err(io::Error::other(
                        "Editor view belongs to a different document",
                    ));
                }
                EditorView::from_existing(&mut session, view)?
            } else {
                let view = EditorView::new(&mut session, document)?;
                if let Err(error) = restore_text_state(&mut session, view.id(), &creation.state) {
                    session.detach_view(view.id());
                    return Err(error);
                }
                view
            };
            Ok(Widget::Text(view))
        }
    })();
    if created && result.is_err() {
        let _ = session.close_document(document, bed_documents::session::ClosePolicy::Discard);
    }
    let widget = result.map_err(|error| error.to_string())?;
    binding.document = Some(document);
    binding.input = bed_api::documents::PanelInput::Document(document);
    binding.session_id = session.session_id().0;
    binding
        .viewer
        .get_or_insert_with(|| if hex { HEX_VIEWER } else { TEXT_VIEWER }.into());
    binding.view = match &widget {
        Widget::Text(view) => Some(view.id()),
        Widget::Hex(_) => None,
    };
    #[cfg(feature = "language-services")]
    if let Widget::Text(view) = &widget {
        world.non_send_mut::<EditorRuntime>().record_overlay(view);
    }
    world
        .non_send_mut::<EditorViews>()
        .widgets
        .insert(creation.entity, widget);
    let systems = world.resource::<Systems>();
    let hooks = (
        PanelActions(systems.actions),
        bed_api::workbench::PanelClipboard(systems.clipboard),
        PanelFocus(systems.focus),
        PanelClose(systems.close),
    );
    world.entity_mut(creation.entity).insert((
        binding,
        bed_api::workbench::PanelPadding([0.0; 2]),
        EditorPanelState {
            document,
            initial_revision,
            hex,
        },
        hooks,
    ));
    #[cfg(feature = "language-services")]
    world
        .entity_mut(creation.entity)
        .insert(LspSymbolDocumentation::default());
    if let Some(revision) = initial_revision {
        world
            .entity_mut(creation.entity)
            .insert(bed_api::documents::PanelDefaultInput { document, revision });
    }
    Ok(new_frontend(creation.entity))
}

/// Persist the live editor view, including selection transforms from edits in siblings.
pub fn save_embedded(world: &World, entity: Entity) -> Value {
    let session = world.non_send::<DocumentSession>();
    match world
        .non_send::<EditorViews>()
        .widgets
        .get(&entity)
        .expect("editor presentation owns a widget")
    {
        Widget::Text(view) => {
            let state = session
                .view_snapshot(view.id())
                .expect("live editor has a session view");
            json!({"selections":state.selections.iter().map(|selection| [selection.head_row,selection.head_column,selection.anchor_row,selection.anchor_column]).collect::<Vec<_>>(),"primary":state.primary_index,"scroll":state.requested_scroll.unwrap_or(state.scroll_position)})
        }
        Widget::Hex(editor) => {
            let mut editor = editor.borrow_mut();
            editor
                .synchronize(session)
                .expect("live hex editor has a session document");
            editor.state()
        }
    }
}

pub fn focus_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    let view = world
        .get::<PanelBinding>(entity)
        .and_then(|binding| binding.view);
    if let Some(view) = view {
        world
            .non_send_mut::<DocumentSession>()
            .request_focus(view)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// A pristine default buffer may be replaced until its first edit. Undoing
/// back to empty contents does not restore that permission.
pub fn input_is_empty(world: &World, entity: Entity) -> bool {
    let Some(state) = world.get::<EditorPanelState>(entity) else {
        return false;
    };
    if let Some(session) = world.get_non_send::<DocumentSession>() {
        return session.document_revision(state.document).ok() == state.initial_revision
            && session.snapshot(state.document).is_ok_and(|document| {
                document.path.is_empty() && document.bytes.is_empty() && !document.dirty
            });
    }
    world.get_resource::<HostFrame>().is_some_and(|frame| {
        input_is_pristine(&frame.context(), state.document, state.initial_revision)
    })
}
