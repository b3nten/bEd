//! Native titlebar/material adapter translated from ned util/macos_window.mm.
//! Winit retains its content view and application delegate; see PORTING.md.
use bed_core::util::color::{ensure_contrast, relative_luminance};
use objc2::{
    ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject},
    sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSButton, NSButtonType, NSCellImagePosition, NSColor, NSFont,
    NSFontWeightSemibold, NSImage, NSLayoutAttribute, NSLayoutConstraint, NSStackView,
    NSStackViewDistribution, NSTextAlignment, NSTextField, NSTitlebarAccessoryViewController,
    NSTitlebarSeparatorStyle, NSUserInterfaceLayoutOrientation, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowButton,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use std::{cell::RefCell, io};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarAction {
    Sidebar,
    Terminal,
    Settings,
    Search,
    Diagnostics,
    Structure,
    SplitRight,
    SplitDown,
}

const CONTROL_WIDTH: f64 = 26.0;
const CONTROL_HEIGHT: f64 = 22.0;
const CONTROL_SPACING: f64 = 2.0;
const CONTROL_COUNT: usize = 8;

#[derive(Default)]
struct Actions {
    pending: RefCell<Vec<TitlebarAction>>,
}

define_class!(
    // Borderless symbol buttons need geometric hit areas. NSButton's default
    // optical alignment insets vary by symbol, so equal constraints otherwise
    // produce differently sized frames and visible gaps.
    #[unsafe(super = NSButton)]
    #[name = "BedToolbarButton"]
    #[thread_kind = MainThreadOnly]
    struct ToolbarButton;
    impl ToolbarButton {
        #[unsafe(method(alignmentRectForFrame:))]
        fn alignment_rect_for_frame(&self, frame: NSRect) -> NSRect { frame }
        #[unsafe(method(frameForAlignmentRect:))]
        fn frame_for_alignment_rect(&self, frame: NSRect) -> NSRect { frame }
    }
);
define_class!(
    // SAFETY: NSObject imposes no subclass requirements; all callbacks run on
    // AppKit's main thread and only queue owned values for the editor loop.
    #[unsafe(super = NSObject)]
    #[name = "BedTitlebarActions"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Actions]
    struct NativeActions;
    unsafe impl NSObjectProtocol for NativeActions {}
    impl NativeActions {
        #[unsafe(method(newExplorer:))]
        fn sidebar(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::Sidebar);
        }
        #[unsafe(method(newTerminal:))]
        fn terminal(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::Terminal);
        }
        #[unsafe(method(newSettings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::Settings);
        }
        #[unsafe(method(newSearch:))]
        fn search(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::Search);
        }
        #[unsafe(method(newDiagnostics:))]
        fn diagnostics(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::Diagnostics);
        }
        #[unsafe(method(newStructure:))]
        fn structure(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::Structure);
        }
        #[unsafe(method(splitRight:))]
        fn split_right(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::SplitRight);
        }
        #[unsafe(method(splitDown:))]
        fn split_down(&self, _sender: Option<&AnyObject>) {
            self.ivars().pending.borrow_mut().push(TitlebarAction::SplitDown);
        }
    }
);
define_class!(
    // SAFETY: No custom view drawing or lifecycle overrides. Returning nil lets
    // pointer events reach the original WinitView, as the source title label does.
    #[unsafe(super = NSTextField)]
    #[name = "BedTitleLabel"]
    #[thread_kind = MainThreadOnly]
    struct TitleLabel;
    impl TitleLabel {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView { std::ptr::null_mut() }
    }
);
define_class!(
    // SAFETY: The material view is a passive background, without event handling.
    #[unsafe(super = NSVisualEffectView)]
    #[name = "BedBackgroundMaterial"]
    #[thread_kind = MainThreadOnly]
    struct BackgroundMaterial;
    impl BackgroundMaterial {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView { std::ptr::null_mut() }
    }
);

pub struct MacOsWindow {
    window: Retained<NSWindow>,
    content: Retained<NSView>,
    material: Retained<BackgroundMaterial>,
    actions: Retained<NativeActions>,
    accessory: Retained<NSTitlebarAccessoryViewController>,
    buttons: Vec<Retained<NSButton>>,
    title: Option<Retained<TitleLabel>>,
    title_width: Option<Retained<NSLayoutConstraint>>,
    title_text: String,
    metal: Option<Retained<CALayer>>,
    original_metal_opacity: f32,
    opacity: f32,
    blur: bool,
    theme: Option<([f32; 4], [f32; 4])>,
}

impl MacOsWindow {
    pub fn configure(window: &Window, opacity: f32, blur: bool) -> io::Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("AppKit requires the main thread"))?;
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err(io::Error::other("window has no AppKit content view"));
        };
        // SAFETY: Winit's borrowed AppKit handle identifies its live NSView. All
        // references are retained and the adapter never replaces/reparents it.
        let content: Retained<NSView> =
            unsafe { handle.ns_view.cast::<NSView>().as_ref().retain() };
        let native = content
            .window()
            .ok_or_else(|| io::Error::other("AppKit content view has no window"))?;
        native.setStyleMask(
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::FullSizeContentView,
        );
        native.setTitlebarAppearsTransparent(true);
        native.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        native.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
        native.setHasShadow(true);
        native.setMovableByWindowBackground(false);
        native.setOpaque(false);
        native.setBackgroundColor(Some(&NSColor::clearColor()));
        native.setAlphaValue(1.0);
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = native.standardWindowButton(kind) {
                button.setHidden(false);
            }
        }
        // SAFETY: Inherited NSView initializer is correct for this view subclass.
        let material: Retained<BackgroundMaterial> =
            unsafe { msg_send![BackgroundMaterial::alloc(mtm), initWithFrame: content.bounds()] };
        material.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        material.setState(NSVisualEffectState::Active);
        material.setMaterial(NSVisualEffectMaterial::HUDWindow);
        material.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        material.setWantsLayer(true);
        content.addSubview(&material);
        // Wgpu creates a Metal sublayer in WinitView's backing layer. Keep the
        // passive material below it; only the GPU sublayer receives opacity.
        if let Some(layer) = material.layer() {
            layer.setZPosition(-1.0);
        }
        let actions = NativeActions::alloc(mtm).set_ivars(Actions::default());
        let actions: Retained<NativeActions> = unsafe { msg_send![super(actions), init] };
        let (accessory, buttons) = install_controls(&native, &actions, mtm);
        let title_text = native.title().to_string();
        let mut result = Self {
            window: native,
            content,
            material,
            actions,
            accessory,
            buttons,
            title: None,
            title_width: None,
            title_text,
            metal: None,
            original_metal_opacity: 1.0,
            opacity: f32::NAN,
            blur: !blur,
            theme: None,
        };
        result.install_title(mtm);
        result.update(opacity, blur)?;
        result.window.makeFirstResponder(Some(&result.content));
        result.window.makeKeyAndOrderFront(None);
        Ok(result)
    }
    pub fn take_actions(&mut self) -> Vec<TitlebarAction> {
        std::mem::take(&mut *self.actions.ivars().pending.borrow_mut())
    }
    pub fn set_title(&mut self, title: &str) {
        if self.title_text == title {
            return;
        }
        let text = NSString::from_str(title);
        self.window.setTitle(&text);
        if let Some(label) = &self.title {
            label.setStringValue(&text);
            label.setToolTip(Some(&text));
        }
        self.title_text = title.to_owned();
    }
    pub fn titlebar_inset(&self) -> f32 {
        let height =
            self.content.bounds().size.height - self.window.contentLayoutRect().size.height;
        if height > 1.0 { height as f32 } else { 28.0 }
    }
    pub fn update(&mut self, opacity: f32, blur: bool) -> io::Result<()> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("AppKit requires the main thread"))?;
        if !self.preserves_winit_content_view() {
            return Err(io::Error::other("AppKit content view identity changed"));
        }
        if self.title.is_none() {
            self.install_title(mtm);
        }
        if let (Some(label), Some(width)) = (&self.title, &self.title_width)
            && let Some(bar) = unsafe { label.superview() }
        {
            let reserved = (self.accessory.view().frame().size.width + 16.0).max(100.0);
            let available = (bar.frame().size.width - 2.0 * reserved).max(0.0);
            if width.constant() != available {
                width.setConstant(available);
            }
        }
        let metal = self
            .content
            .layer()
            .and_then(|root| find_metal_layer(&root));
        let changed_layer = match (&self.metal, &metal) {
            (Some(old), Some(new)) => !std::ptr::eq(&**old, &**new),
            (None, None) => false,
            _ => true,
        };
        if changed_layer {
            self.original_metal_opacity = metal.as_ref().map_or(1.0, |layer| layer.opacity());
            self.metal = metal;
        }
        let opacity = opacity.clamp(0.0, 1.0);
        if changed_layer || opacity != self.opacity || blur != self.blur {
            if let Some(layer) = &self.metal {
                layer.setOpacity(opacity);
            }
            self.material.setHidden(!blur);
            self.material.setNeedsDisplay(true);
            self.window.invalidateShadow();
            self.window.displayIfNeeded();
            self.window.setHasShadow(false);
            self.window.setHasShadow(true);
            self.opacity = opacity;
            self.blur = blur;
        }
        Ok(())
    }
    pub fn update_theme(&mut self, text: [f32; 4], background: [f32; 4]) -> io::Result<()> {
        let text = ensure_contrast(text, background, 4.5);
        if self.theme == Some((text, background)) {
            return Ok(());
        }
        set_native_appearance(&self.window, background)?;
        let color = native_color(text);
        if let Some(title) = &self.title {
            title.setTextColor(Some(&color));
        }
        for button in &self.buttons {
            button.setContentTintColor(Some(&color));
            NSView::setNeedsDisplay(button, true);
        }
        self.theme = Some((text, background));
        Ok(())
    }
    /// Detached viewports inherit the document theme instead of the OS theme.
    pub fn apply_theme_to_window(
        window: &Window,
        text: [f32; 4],
        background: [f32; 4],
    ) -> io::Result<()> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err(io::Error::other("window has no AppKit content view"));
        };
        let content = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let native = content
            .window()
            .ok_or_else(|| io::Error::other("AppKit content view has no window"))?;
        set_native_appearance(&native, background)?;
        if let Some(frame) = unsafe { content.superview() } {
            tint_native_title(
                &frame,
                &native_color(ensure_contrast(text, background, 4.5)),
            );
        }
        Ok(())
    }
    pub fn preserves_winit_content_view(&self) -> bool {
        self.window
            .contentView()
            .is_some_and(|view| std::ptr::eq(&*view, &*self.content))
    }
    pub fn material_is_visible(&self) -> bool {
        !self.material.isHidden()
    }
    pub fn content_opacity(&self) -> Option<f32> {
        self.metal.as_ref().map(|layer| layer.opacity())
    }
    /// Actual AppKit rectangles in the accessory's coordinate space, after layout.
    pub fn control_frames(&self) -> Vec<[f64; 4]> {
        let view = self.accessory.view();
        view.layoutSubtreeIfNeeded();
        self.buttons
            .iter()
            .map(|button| {
                let frame = button.convertRect_toView(button.bounds(), Some(&view));
                [
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                ]
            })
            .collect()
    }
    /// Native acceptance checks use rendered geometry, rather than stack settings.
    pub fn validate_control_layout(&self) -> io::Result<()> {
        if self.window.title().to_string() != self.title_text
            || self
                .title
                .as_ref()
                .is_none_or(|label| label.stringValue().to_string() != self.title_text)
        {
            return Err(io::Error::other(
                "native window title and visible caption differ",
            ));
        }
        let frames = self.control_frames();
        if frames.len() != CONTROL_COUNT
            || self.buttons.iter().any(|button| button.image().is_none())
        {
            return Err(io::Error::other(
                "native toolbar is missing a control or icon",
            ));
        }
        for frame in &frames {
            if (frame[2] - CONTROL_WIDTH).abs() > 0.25
                || (frame[3] - CONTROL_HEIGHT).abs() > 0.25
                || (frame[1] - frames[0][1]).abs() > 0.25
            {
                return Err(io::Error::other(format!(
                    "native toolbar control has unequal geometry: {frames:?}"
                )));
            }
        }
        for pair in frames.windows(2) {
            if (pair[1][0] - pair[0][0] - pair[0][2] - CONTROL_SPACING).abs() > 0.25 {
                return Err(io::Error::other(format!(
                    "native toolbar control spacing is unequal: {frames:?}"
                )));
            }
        }
        Ok(())
    }
    /// Invoke the native control through AppKit's target/action path for smoke tests.
    pub fn click_control(&self, action: TitlebarAction) {
        let index = match action {
            TitlebarAction::Sidebar => 0,
            TitlebarAction::Terminal => 1,
            TitlebarAction::Search => 2,
            TitlebarAction::Structure => 3,
            TitlebarAction::Diagnostics => 4,
            TitlebarAction::SplitRight => 5,
            TitlebarAction::SplitDown => 6,
            TitlebarAction::Settings => 7,
        };
        // SAFETY: Retained buttons have valid retained targets and exact selectors.
        unsafe {
            self.buttons[index].performClick(None);
        }
    }
    fn install_title(&mut self, mtm: MainThreadMarker) {
        let Some(bar) = self
            .window
            .standardWindowButton(NSWindowButton::CloseButton)
            .and_then(|button| unsafe { button.superview() })
        else {
            return;
        };
        let label: Retained<TitleLabel> =
            unsafe { msg_send![TitleLabel::alloc(mtm), initWithFrame: NSRect::ZERO] };
        unsafe {
            let _: () = msg_send![&label, setIdentifier: ns_string!("bed.title")];
        }
        label.setStringValue(&NSString::from_str(&self.title_text));
        label.setToolTip(Some(&NSString::from_str(&self.title_text)));
        label.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe {
            NSFontWeightSemibold
        })));
        if let Some((text, _)) = self.theme {
            label.setTextColor(Some(&native_color(text)));
        } else {
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        label.setAlignment(NSTextAlignment::Center);
        label.setEditable(false);
        label.setSelectable(false);
        label.setDrawsBackground(false);
        label.setBordered(false);
        label.setRefusesFirstResponder(true);
        label.setTranslatesAutoresizingMaskIntoConstraints(false);
        bar.addSubview(&label);
        label
            .centerXAnchor()
            .constraintEqualToAnchor(&bar.centerXAnchor())
            .setActive(true);
        label
            .centerYAnchor()
            .constraintEqualToAnchor(&bar.centerYAnchor())
            .setActive(true);
        let reserved = (self.accessory.view().frame().size.width + 16.0).max(100.0);
        let width = label.widthAnchor().constraintLessThanOrEqualToConstant(
            (bar.frame().size.width - 2.0 * reserved).max(0.0),
        );
        width.setActive(true);
        self.title_width = Some(width);
        self.title = Some(label);
    }
}
impl Drop for MacOsWindow {
    fn drop(&mut self) {
        // The host's view/delegate are never replaced. Remove only our own views.
        if let Some(layer) = &self.metal {
            layer.setOpacity(self.original_metal_opacity);
        }
        if let Some(label) = &self.title {
            label.removeFromSuperview();
        }
        self.material.removeFromSuperview();
        for index in (0..self.window.titlebarAccessoryViewControllers().len()).rev() {
            if std::ptr::eq(
                &*self
                    .window
                    .titlebarAccessoryViewControllers()
                    .objectAtIndex(index),
                &*self.accessory,
            ) {
                self.window
                    .removeTitlebarAccessoryViewControllerAtIndex(index as _);
            }
        }
    }
}

fn native_color(color: [f32; 4]) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        color[0] as f64,
        color[1] as f64,
        color[2] as f64,
        color[3] as f64,
    )
}
fn tint_native_title(view: &NSView, color: &NSColor) {
    let object: &AnyObject = view;
    if let Some(field) = object.downcast_ref::<NSTextField>() {
        field.setTextColor(Some(color));
    }
    for child in view.subviews() {
        tint_native_title(&child, color);
    }
}
fn set_native_appearance(window: &NSWindow, background: [f32; 4]) -> io::Result<()> {
    MainThreadMarker::new().ok_or_else(|| io::Error::other("AppKit requires the main thread"))?;
    let class = AnyClass::get(c"NSAppearance")
        .ok_or_else(|| io::Error::other("NSAppearance is unavailable"))?;
    let name = if relative_luminance(background) > 0.179 {
        "NSAppearanceNameAqua"
    } else {
        "NSAppearanceNameDarkAqua"
    };
    // Stable public AppKit selectors, used on the main thread with retained objects.
    let appearance: Option<Retained<AnyObject>> =
        unsafe { msg_send![class, appearanceNamed: &*NSString::from_str(name)] };
    if let Some(appearance) = appearance {
        unsafe {
            let _: () = msg_send![window, setAppearance: &*appearance];
        }
    }
    window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
    Ok(())
}
fn find_metal_layer(root: &CALayer) -> Option<Retained<CALayer>> {
    if root.isKindOfClass(CAMetalLayer::class()) {
        return Some(root.retain());
    }
    unsafe { root.sublayers() }?
        .iter()
        .find_map(|layer| find_metal_layer(&layer))
}
fn titlebar_symbol(names: &[&str]) -> Option<Retained<NSImage>> {
    for name in names {
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        ) {
            image.setTemplate(true);
            // SAFETY: AppKit's NSImageSymbolConfiguration point-size/weight factory
            // uses CGFloat arguments; NSFontWeightRegular is 0.0.
            let configuration_class = objc2::runtime::AnyClass::get(c"NSImageSymbolConfiguration")?;
            let configuration: Retained<AnyObject> = unsafe {
                msg_send![configuration_class, configurationWithPointSize: 13.0f64, weight: 0.0f64]
            };
            let sized: Option<Retained<NSImage>> =
                unsafe { msg_send![&image, imageWithSymbolConfiguration: &*configuration] };
            let image = sized.unwrap_or(image);
            image.setTemplate(true);
            return Some(image);
        }
    }
    None
}
fn install_controls(
    window: &NSWindow,
    target: &NativeActions,
    mtm: MainThreadMarker,
) -> (
    Retained<NSTitlebarAccessoryViewController>,
    Vec<Retained<NSButton>>,
) {
    let mut buttons = Vec::new();
    for (symbols, tooltip, action) in [
        (
            &["sidebar.left", "rectangle.split.1x2"][..],
            "New File Explorer",
            sel!(newExplorer:),
        ),
        (
            &[
                "rectangle.bottomhalf.inset.filled",
                "rectangle.bottomhalf.filled",
                "dock.rectangle",
            ][..],
            "New Terminal",
            sel!(newTerminal:),
        ),
        (
            &["magnifyingglass"][..],
            "New Project Search",
            sel!(newSearch:),
        ),
        (
            &["list.bullet.indent", "list.bullet"][..],
            "New Structure",
            sel!(newStructure:),
        ),
        (
            &["exclamationmark.triangle", "exclamationmark.circle"][..],
            "New Diagnostics",
            sel!(newDiagnostics:),
        ),
        (
            &["rectangle.split.2x1", "square.split.2x1"][..],
            "Split Editor Right",
            sel!(splitRight:),
        ),
        (
            &["rectangle.split.1x2", "square.split.1x2"][..],
            "Split Editor Down",
            sel!(splitDown:),
        ),
        (
            &["gearshape", "gear"][..],
            "New Settings",
            sel!(newSettings:),
        ),
    ] {
        // SAFETY: Inherited NSButton initializer leaves the subclass's only
        // customization (identity alignment rectangles) intact.
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(CONTROL_WIDTH, CONTROL_HEIGHT));
        let button: Retained<ToolbarButton> =
            unsafe { msg_send![ToolbarButton::alloc(mtm), initWithFrame: frame] };
        let button = button.into_super();
        button.setButtonType(NSButtonType::MomentaryChange);
        button.setBordered(false);
        button.setImage(titlebar_symbol(symbols).as_deref());
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        unsafe {
            button.setTarget(Some(target));
            button.setAction(Some(action));
        }
        button.setToolTip(Some(&NSString::from_str(tooltip)));
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        button
            .widthAnchor()
            .constraintEqualToConstant(CONTROL_WIDTH)
            .setActive(true);
        button
            .heightAnchor()
            .constraintEqualToConstant(CONTROL_HEIGHT)
            .setActive(true);
        buttons.push(button);
    }
    let views: Vec<&NSView> = buttons.iter().map(|button| -> &NSView { button }).collect();
    let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(&views), mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setAlignment(NSLayoutAttribute::CenterY);
    stack.setDistribution(NSStackViewDistribution::Fill);
    stack.setSpacing(CONTROL_SPACING);
    stack.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: 4.0,
        bottom: 0.0,
        right: 10.0,
    });
    let width =
        CONTROL_WIDTH * CONTROL_COUNT as f64 + CONTROL_SPACING * (CONTROL_COUNT - 1) as f64 + 14.0;
    stack.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, 28.0)));
    let accessory = NSTitlebarAccessoryViewController::new(mtm);
    accessory.setView(&stack);
    accessory.setLayoutAttribute(NSLayoutAttribute::Trailing);
    window.addTitlebarAccessoryViewController(&accessory);
    (accessory, buttons)
}
