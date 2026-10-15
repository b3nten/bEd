//! Run on macOS's main thread: menu state changes preserve native objects and
//! deliver each action once, including after a structural replacement.
#[cfg(target_os = "macos")]
fn main() -> std::io::Result<()> {
    use bedtime_desktop::{
        DesktopMenu, DesktopMenuCommand, DesktopMenuItem, native::macos_menu::MacOsMenu,
    };
    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::NSApplication;

    let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
    let previous = app.mainMenu();
    let definitions = |selected, enabled, label: &str| {
        vec![DesktopMenu {
            label: "Window".into(),
            windows_menu: false,
            items: vec![DesktopMenuItem::Submenu {
                label: label.into(),
                items: (0..3)
                    .map(|index| {
                        let command = DesktopMenuCommand {
                            id: format!("layout.{index}"),
                            label: format!("Layout {index}"),
                            enabled,
                            shortcut: None,
                        };
                        if index == selected {
                            DesktopMenuItem::SelectedCommand(command)
                        } else {
                            DesktopMenuItem::Command(command)
                        }
                    })
                    .collect(),
            }],
        }]
    };
    let mut menu = MacOsMenu::install(&definitions(0, true, "Layout"), &[])?;
    let root = app.mainMenu().unwrap();
    let layouts = root
        .itemAtIndex(0)
        .unwrap()
        .submenu()
        .unwrap()
        .itemAtIndex(0)
        .unwrap()
        .submenu()
        .unwrap();
    let items = layouts.itemArray();
    for round in 0..100 {
        let selected = round % 3;
        let enabled = round % 2 == 0;
        menu.update(&definitions(selected, enabled, "Layout"), &[])?;
        assert_eq!(
            Retained::as_ptr(&app.mainMenu().unwrap()),
            Retained::as_ptr(&root),
            "changing a checkmark or availability replaced the native menu"
        );
        for (index, item) in items.iter().enumerate() {
            assert_eq!(item.state(), isize::from(index == selected));
            assert_eq!(item.isEnabled(), enabled);
        }
        menu.update(&definitions(selected, true, "Layout"), &[])?;
        let id = format!("layout.{selected}");
        menu.perform_for_smoke(&id)?;
        let actions = menu.poll();
        assert_eq!(actions.len(), 1, "menu action was lost or duplicated");
        assert_eq!(actions[0].command, id);
        assert!(menu.poll().is_empty());
    }
    menu.update(&definitions(0, true, "Saved Layouts"), &[])?;
    assert_ne!(
        Retained::as_ptr(&app.mainMenu().unwrap()),
        Retained::as_ptr(&root)
    );
    menu.perform_for_smoke("layout.0")?;
    assert_eq!(menu.poll().len(), 1);
    drop(menu);
    assert_eq!(app.mainMenu(), previous);
    println!("macOS menu smoke PASS: 100 state changes and actions, replacement and teardown");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("macOS menu smoke requires macOS");
}
