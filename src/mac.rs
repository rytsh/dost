//! macOS shell: a borderless, transparent, always-on-top window for the dog above the Dock,
//! a menu bar icon (left click: settings panel, right click: short menu) and the settings
//! panel window. Behaviour lives in [`dost::app::Core`].
//!
//! Frames are rendered with tiny-skia into a pixmap and shown through an `NSImageView`.
//! AppKit uses a y-up coordinate system with the origin at the bottom-left of the main
//! screen; the core uses y-down physical pixels, so everything here converts between them.

use std::cell::RefCell;
use std::path::Path;
use std::ptr::NonNull;

use block2::RcBlock;
use muda::{ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapFormat,
    NSBitmapImageRep, NSCellImagePosition, NSColor, NSEvent, NSEventMask, NSEventType, NSImage,
    NSImageScaling, NSImageView, NSMenu, NSPasteboard, NSPasteboardTypeString, NSScreen, NSSound,
    NSStatusBar, NSStatusBarButton, NSStatusItem, NSStatusWindowLevel, NSVariableStatusItemLength,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSData, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSURL,
};
use tiny_skia::Pixmap;

use dost::app::{self, Core, EDGE, Platform};
use dost::dog::Bounds;
use dost::panel::{self, Panel};
use dost::render::{Renderer, Scale};

const FRAME: f64 = 1.0 / 60.0;
const DRAG_THRESHOLD: f64 = 4.0;
/// Gap between the panel and the menu bar / screen edge, points.
const PANEL_MARGIN: f64 = 8.0;

/// A rectangle in the core's coordinates: physical pixels, y down from the top of the
/// main screen.
#[derive(Clone, Copy, Debug)]
struct PxRect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    /// Backing scale factor of the screen (2.0 on Retina).
    scale: f64,
}

struct Drag {
    start: NSPoint,
    grab_dx: f32,
    grab_dy: f32,
    moved: bool,
}

struct App {
    core: Core,
    shell: Shell,
}

struct Shell {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    view: Retained<NSImageView>,
    pixmap: Pixmap,
    /// The dog window's top-left on screen (physical).
    dog_pos: (f64, f64),
    drag: Option<Drag>,
    status: Retained<NSStatusItem>,
    /// Whether the menu bar icon is drawn pressed (while the panel is open).
    status_pressed: bool,
    /// What the menu bar icon shows: (sleeping dog, unread mark).
    status_icon: (bool, bool),
    menu: Menu,
    items: MenuItems,
    panel: PanelWindow,
    /// The big settings window (titled, resizable), while open.
    settings: SettingsWindow,
    hotkeys: crate::hotkeys::Hotkeys,
    /// While hidden: the bubble on its own under the menu bar icon.
    alone: AloneBubble,
    /// The bone's window.
    toy: ToyWindow,
    sound: Option<Retained<NSSound>>,
    quit: bool,
}

/// A small window for the toy, so it can fly anywhere above the Dock.
struct ToyWindow {
    window: Retained<NSWindow>,
    view: Retained<NSImageView>,
    pixmap: Pixmap,
    shown: bool,
    /// Cursor offset from the toy's bottom-centre while dragging (physical).
    drag: Option<(f32, f32)>,
}

/// A bubble window without the dog, hanging under the menu bar icon.
struct AloneBubble {
    window: Retained<NSWindow>,
    view: Retained<NSImageView>,
    pixmap: Pixmap,
    shown: bool,
    /// The mouse went down on it (a click needs both down and up here).
    pressed: bool,
    /// Top-left on screen (physical) and the screen's scale, for click positions.
    pos: (f64, f64, f64),
}

struct MenuItems {
    show: MenuItem,
    mute: MenuItem,
    settings: MenuItem,
    quit: MenuItem,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let ns_app = NSApplication::sharedApplication(mtm);
    // Menu bar app: no Dock icon, no app menu.
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let renderer = Renderer::new(true, Vec::new());
    let (menu, items) = build_menu();
    let status = make_status_item(mtm, &renderer);

    let (window, view) = make_window(mtm, false);
    window.setIgnoresMouseEvents(false);

    let screen = main_screen(mtm);
    let start_x = screen.left + (screen.right - screen.left) * 0.75;
    let core = Core::new(renderer, start_x as f32, screen.bottom as f32);
    // Hidden last time: stays hidden.
    if core.on_screen() {
        window.orderFrontRegardless();
    }
    let shell = Shell {
        mtm,
        window,
        view,
        pixmap: Pixmap::new(1, 1).unwrap(),
        dog_pos: (0.0, 0.0),
        drag: None,
        status,
        status_pressed: false,
        status_icon: (false, false),
        menu,
        items,
        panel: PanelWindow::default(),
        settings: SettingsWindow::default(),
        hotkeys: crate::hotkeys::Hotkeys::new(),
        alone: {
            let (window, view) = make_window(mtm, false);
            AloneBubble {
                window,
                view,
                pixmap: Pixmap::new(1, 1).unwrap(),
                shown: false,
                pressed: false,
                pos: (0.0, 0.0, 1.0),
            }
        },
        toy: {
            let (window, view) = make_window(mtm, false);
            ToyWindow {
                window,
                view,
                pixmap: Pixmap::new(1, 1).unwrap(),
                shown: false,
                drag: None,
            }
        },
        sound: None,
        quit: false,
    };
    APP.with(|a| *a.borrow_mut() = Some(App { core, shell }));
    with_app(App::sync_menu);

    install_mouse_monitor();
    let tick = RcBlock::new(|_timer: NonNull<NSTimer>| {
        let quit = with_app(|a| {
            a.tick();
            if a.shell.quit {
                a.core.shutdown();
            }
            a.shell.quit
        });
        if quit == Some(true) {
            let mtm = MainThreadMarker::new().unwrap();
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    });
    // Common modes: keep ticking while the menu bar menu is open (it tracks in its own mode).
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(FRAME, true, &tick) };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    ns_app.run();
}

/// Menu bar icon. Clicks are handled by the event monitor: left opens the settings panel
/// below it, right pops up the short menu.
fn make_status_item(mtm: MainThreadMarker, renderer: &Renderer) -> Retained<NSStatusItem> {
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        set_status_icon(&button, renderer, (false, false));
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        button.setToolTip(Some(&NSString::from_str("Dost")));
    }
    item
}

fn set_status_icon(
    button: &NSStatusBarButton,
    renderer: &Renderer,
    (sleeping, unread): (bool, bool),
) {
    let icon = renderer.icon_pixmap(44, sleeping, unread);
    button.setImage(to_image(&icon, NSSize::new(22.0, 22.0)).as_deref());
}

/// Pops up the short menu under the menu bar icon. Runs a modal loop until it closes, so it
/// must not be called while [`APP`] is borrowed.
fn show_status_menu() {
    let Some((item, menu)) = with_app(|a| {
        let menu = unsafe { Retained::retain(a.shell.menu.ns_menu().cast::<NSMenu>()) };
        a.shell.panel.close();
        (a.shell.status.clone(), menu)
    }) else {
        return;
    };
    if let Some(menu) = menu {
        // Deprecated in favour of `menu`, but a permanent menu would also take left clicks.
        #[allow(deprecated)]
        item.popUpStatusItemMenu(&menu);
    }
}

/// Bottom-center of the menu bar icon in core coordinates.
fn status_anchor(mtm: MainThreadMarker, item: &NSStatusItem) -> Option<(f64, f64)> {
    let window = item.button(mtm)?.window()?;
    let f = window.frame();
    let k = window.backingScaleFactor();
    Some((
        (f.origin.x + f.size.width / 2.0) * k,
        (main_height(mtm) - f.origin.y) * k,
    ))
}

fn build_menu() -> (Menu, MenuItems) {
    let items = MenuItems {
        show: MenuItem::with_id("show", "Hide", true, None),
        mute: MenuItem::with_id("mute", "Mute", true, None),
        settings: MenuItem::with_id("settings", "All settings…", true, None),
        quit: MenuItem::with_id("quit", "Quit Dost", true, None),
    };
    let menu = Menu::new();
    menu.append_items(&[
        &items.settings,
        &PredefinedMenuItem::separator(),
        &items.show,
        &items.mute,
        &PredefinedMenuItem::separator(),
        &items.quit,
    ])
    .expect("menu");
    (menu, items)
}

/// A borderless, transparent, shadowless window with an image view filling it.
fn make_window(mtm: MainThreadMarker, panel: bool) -> (Retained<NSWindow>, Retained<NSImageView>) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0));
    let style = NSWindowStyleMask::Borderless;
    let backing = NSBackingStoreType::Buffered;
    let window: Retained<NSWindow> = if panel {
        // The panel takes the keyboard (the name field); borderless windows do not by default.
        let w: Retained<KeyWindow> = unsafe {
            msg_send![
                KeyWindow::alloc(mtm),
                initWithContentRect: frame,
                styleMask: style,
                backing: backing,
                defer: false
            ]
        };
        w.into_super()
    } else {
        unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                backing,
                false,
            )
        }
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(panel);
    window.setLevel(NSStatusWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    window.setAcceptsMouseMovedEvents(true);
    let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
    view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
    window.setContentView(Some(&view));
    (window, view)
}

define_class!(
    // SAFETY: NSWindow has no subclassing requirements; KeyWindow has no Drop.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "DostKeyWindow"]
    struct KeyWindow;

    impl KeyWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }
    }
);

/// Converts a pixmap (premultiplied RGBA) to an `NSImage` of `points` size.
fn to_image(pm: &Pixmap, points: NSSize) -> Option<Retained<NSImage>> {
    let (w, h) = (pm.width() as isize, pm.height() as isize);
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            &NSString::from_str("NSDeviceRGBColorSpace"),
            NSBitmapFormat::empty(),
            w * 4,
            32,
        )?
    };
    let dst = rep.bitmapData();
    if dst.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(pm.data().as_ptr(), dst, pm.data().len()) };
    rep.setSize(points);
    let image = NSImage::initWithSize(NSImage::alloc(), points);
    image.addRepresentation(&rep);
    Some(image)
}

/// Every screen as (visible area, full area) in core coordinates.
fn screens(mtm: MainThreadMarker) -> Vec<(PxRect, PxRect)> {
    let all = NSScreen::screens(mtm);
    let Some(main) = all.firstObject() else {
        return Vec::new();
    };
    let main_h = main.frame().size.height;
    all.iter()
        .map(|s| {
            let k = s.backingScaleFactor();
            let conv = |r: NSRect| PxRect {
                left: r.origin.x * k,
                top: (main_h - r.origin.y - r.size.height) * k,
                right: (r.origin.x + r.size.width) * k,
                bottom: (main_h - r.origin.y) * k,
                scale: k,
            };
            (conv(s.visibleFrame()), conv(s.frame()))
        })
        .collect()
}

fn main_screen(mtm: MainThreadMarker) -> PxRect {
    screens(mtm).first().map_or(
        PxRect {
            left: 0.0,
            top: 0.0,
            right: 1440.0,
            bottom: 900.0,
            scale: 1.0,
        },
        |(visible, _)| *visible,
    )
}

/// The screen containing the physical point (x, y), or the nearest one.
fn screen_at(mtm: MainThreadMarker, x: f64, y: f64) -> (PxRect, PxRect) {
    let all = screens(mtm);
    let dist = |r: &PxRect| {
        let dx = (r.left - x).max(0.0).max(x - r.right);
        let dy = (r.top - y).max(0.0).max(y - r.bottom);
        dx * dx + dy * dy
    };
    all.iter()
        .copied()
        .min_by(|a, b| dist(&a.1).total_cmp(&dist(&b.1)))
        .unwrap_or((main_screen(mtm), main_screen(mtm)))
}

/// The login window's "screen is locked" flag for this session.
fn screen_locked() -> bool {
    use std::ffi::c_void;
    type Cf = *const c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> Cf;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDictionaryGetValue(d: Cf, key: Cf) -> Cf;
        fn CFStringCreateWithCString(alloc: Cf, s: *const std::ffi::c_char, enc: u32) -> Cf;
        fn CFBooleanGetValue(b: Cf) -> bool;
        fn CFGetTypeID(cf: Cf) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFRelease(cf: Cf);
    }
    const UTF8: u32 = 0x0800_0100;
    unsafe {
        let d = CGSessionCopyCurrentDictionary();
        if d.is_null() {
            return false;
        }
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"CGSSessionScreenIsLocked".as_ptr(), UTF8);
        let v = CFDictionaryGetValue(d, key);
        let locked = !v.is_null() && CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue(v);
        CFRelease(key);
        CFRelease(d);
        locked
    }
}

/// On-screen, normal-level windows of process `pid`: (x, y, w, h) in points, y down.
fn front_windows(pid: i32) -> Vec<(f64, f64, f64, f64)> {
    use std::ffi::c_void;
    type Cf = *const c_void;
    #[repr(C)]
    struct CgRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Cf;
        fn CGRectMakeWithDictionaryRepresentation(dict: Cf, rect: *mut CgRect) -> bool;
        static kCGWindowOwnerPID: Cf;
        static kCGWindowLayer: Cf;
        static kCGWindowBounds: Cf;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(a: Cf) -> isize;
        fn CFArrayGetValueAtIndex(a: Cf, i: isize) -> Cf;
        fn CFDictionaryGetValue(d: Cf, key: Cf) -> Cf;
        fn CFNumberGetValue(n: Cf, kind: isize, out: *mut c_void) -> bool;
        fn CFRelease(cf: Cf);
    }
    // kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements
    const OPTIONS: u32 = 1 | 16;
    const SINT32: isize = 3;
    let mut out = Vec::new();
    unsafe {
        let list = CGWindowListCopyWindowInfo(OPTIONS, 0);
        if list.is_null() {
            return out;
        }
        let int = |d: Cf, k: Cf| {
            let v = CFDictionaryGetValue(d, k);
            let mut n: i32 = -1;
            if !v.is_null() {
                CFNumberGetValue(v, SINT32, (&raw mut n).cast());
            }
            n
        };
        for i in 0..CFArrayGetCount(list) {
            let w = CFArrayGetValueAtIndex(list, i);
            if int(w, kCGWindowOwnerPID) != pid || int(w, kCGWindowLayer) != 0 {
                continue;
            }
            let b = CFDictionaryGetValue(w, kCGWindowBounds);
            let mut r = CgRect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            };
            if !b.is_null() && CGRectMakeWithDictionaryRepresentation(b, &mut r) {
                out.push((r.x, r.y, r.w, r.h));
            }
        }
        CFRelease(list);
    }
    out
}

/// Main-screen height in points (AppKit y flips around it).
fn main_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(900.0, |s| s.frame().size.height)
}

/// Places `window` so its top-left is at physical (x, y) and it is `w`×`h` physical pixels.
fn place(mtm: MainThreadMarker, window: &NSWindow, x: f64, y: f64, w: f64, h: f64, scale: f64) {
    let (pw, ph) = (w / scale, h / scale);
    let origin = NSPoint::new(x / scale, main_height(mtm) - y / scale - ph);
    window.setFrame_display(NSRect::new(origin, NSSize::new(pw, ph)), false);
}

/// The mouse in core coordinates (physical pixels, y down).
fn mouse_px(mtm: MainThreadMarker) -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    let y = main_height(mtm) - p.y;
    let (_, full) = screen_at(mtm, p.x, y);
    (p.x * full.scale, y * full.scale)
}

impl Platform for Shell {
    fn play_wav(&mut self, wav: Vec<u8>) {
        let data = NSData::with_bytes(&wav);
        if let Some(old) = self.sound.take() {
            old.stop();
        }
        if let Some(s) = NSSound::initWithData(NSSound::alloc(), &data) {
            s.play();
            self.sound = Some(s);
        }
    }

    fn copy_text(&mut self, text: &str) -> bool {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    }

    fn autostart_enabled(&self) -> bool {
        autostart::enabled()
    }

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        autostart::set(on)
    }

    fn open_folder(&mut self, path: &Path) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSWorkspace::sharedWorkspace().openURL(&url);
    }

    fn open_url(&mut self, url: &str) {
        if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
            NSWorkspace::sharedWorkspace().openURL(&u);
        }
    }

    fn idle_secs(&self) -> Option<u64> {
        // kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType.
        unsafe extern "C" {
            fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
        }
        let s = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
        s.is_finite().then_some(s.max(0.0) as u64)
    }

    fn fullscreen(&self) -> bool {
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return false;
        };
        let pid = app.processIdentifier();
        if pid == std::process::id() as i32 {
            return false;
        }
        let screens: Vec<NSRect> = NSScreen::screens(self.mtm)
            .iter()
            .map(|s| s.frame())
            .collect();
        let Some(main_h) = screens.first().map(|r| r.size.height) else {
            return false;
        };
        front_windows(pid).iter().any(|w| {
            screens.iter().any(|s| {
                // Window bounds are y-down from the main screen's top; screens are y-up.
                let top = main_h - s.origin.y - s.size.height;
                (w.0 - s.origin.x).abs() < 1.0
                    && (w.1 - top).abs() < 1.0
                    && (w.2 - s.size.width).abs() < 1.0
                    && (w.3 - s.size.height).abs() < 1.0
            })
        })
    }

    fn screen_locked(&self) -> bool {
        screen_locked()
    }

    fn system_notify(&mut self, title: &str, body: &str) {
        // An unsigned app cannot use the UserNotifications framework; osascript can post.
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"{}\" sound name \"Ping\"",
            esc(body),
            esc(title)
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn set_dog_visible(&mut self, visible: bool) {
        if visible {
            self.window.orderFrontRegardless();
        } else {
            self.window.orderOut(None);
        }
    }

    fn close_panel(&mut self) {
        self.panel.close();
    }

    fn quit(&mut self) {
        self.quit = true;
    }

    fn autostart_label(&self) -> &'static str {
        "Open at login"
    }

    fn open_settings_window(&mut self) {
        self.settings.open(self.mtm);
    }

    fn open_reply(&mut self, id: u64, title: &str, question: &str) {
        if !self.panel.is_open() {
            let anchor = status_anchor(self.mtm, &self.status);
            let at = anchor.unwrap_or_else(|| mouse_px(self.mtm));
            self.panel.open(self.mtm, at, anchor.is_some());
        }
        if let Some(p) = self.panel.panel.as_mut() {
            p.start_reply(id, title, question);
        }
        self.panel.redraw = true;
        // Typing goes to the panel.
        if let Some(w) = self.panel.window.as_ref() {
            NSApplication::sharedApplication(self.mtm).activate();
            w.makeKeyAndOrderFront(None);
        }
    }

    fn screen_count(&self) -> usize {
        NSScreen::screens(self.mtm).len()
    }
}

impl App {
    /// Keyboard shortcuts for the bubble: on only while one is shown.
    fn keys(&mut self) {
        for k in self.shell.hotkeys.pressed() {
            self.core.on_key(&mut self.shell, k);
        }
        let shown = self.core.shown.is_some();
        self.shell.hotkeys.set_active(shown);
    }

    fn tick(&mut self) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            self.on_menu(ev.id.as_ref());
        }
        self.drag_update();
        self.toy_drag_update();
        let bounds = self.bounds();
        self.core.tick(&mut self.shell, &bounds);
        self.keys();
        if self.core.on_screen() {
            self.paint();
        }
        self.paint_toy();
        self.paint_alone();
        if self.core.panel_dirty {
            self.core.panel_dirty = false;
            self.sync_menu();
        }
        self.paint_panel();
        self.paint_settings();
        self.shell.panel.animate();
        let open = self.shell.panel.is_open();
        if open != self.shell.status_pressed {
            self.shell.status_pressed = open;
            if let Some(b) = self.shell.status.button(self.shell.mtm) {
                b.highlight(open);
            }
        }
        let icon = (self.core.dog.is_sleeping(), self.core.missed > 0);
        if icon != self.shell.status_icon {
            self.shell.status_icon = icon;
            if let Some(b) = self.shell.status.button(self.shell.mtm) {
                set_status_icon(&b, &self.core.renderer, icon);
            }
        }
    }

    /// The screen the dog is on, or the one it lives on (settings → Screen).
    fn dog_screen(&self) -> (PxRect, PxRect) {
        let n = self.core.settings.screen as usize;
        if n > 0
            && let Some(s) = screens(self.shell.mtm).get(n - 1)
        {
            return *s;
        }
        screen_at(
            self.shell.mtm,
            self.core.dog.x as f64,
            self.core.dog.y as f64 - 10.0,
        )
    }

    fn bounds(&self) -> Bounds {
        let (visible, full) = self.dog_screen();
        let ui = full.scale as f32;
        let px = Scale::new(ui, self.core.settings.size).px as f32;
        Bounds {
            // The visible frame excludes the Dock and the menu bar: its bottom is the Dock's top.
            ground: app::ground_line(
                &self.core.settings,
                visible.bottom as f32,
                full.top as f32,
                full.bottom as f32,
                ui,
                px,
            ),
            left: visible.left as f32 + EDGE * px,
            right: visible.right as f32 - EDGE * px,
            scale: px,
        }
    }

    fn paint(&mut self) {
        let (_, full) = self.dog_screen();
        let scale = self.core.scale(full.scale as f32);
        let frame = self.core.dog.frame();
        let core = &mut self.core;
        let bubble = app::bubble_of(core.shown.as_ref().filter(|_| core.visible), core.waiting());
        let (w, h) = core.renderer.size(frame.anim, bubble.as_ref(), scale);
        let sh = &mut self.shell;
        if sh.pixmap.width() != w || sh.pixmap.height() != h {
            sh.pixmap = Pixmap::new(w, h).unwrap();
        }
        let x = (core.dog.x as f64 - w as f64 / 2.0)
            .clamp(full.left, (full.right - w as f64).max(full.left))
            .round();
        let y = (core.dog.y - Renderer::ground_offset(h, scale)).round() as f64;
        core.hits = core.renderer.draw(
            &mut sh.pixmap,
            core.dog.x - x as f32,
            &frame,
            bubble.as_ref(),
            scale,
        );
        let points = NSSize::new(w as f64 / full.scale, h as f64 / full.scale);
        if let Some(image) = to_image(&sh.pixmap, points) {
            sh.view.setImage(Some(&image));
        }
        place(sh.mtm, &sh.window, x, y, w as f64, h as f64, full.scale);
        sh.dog_pos = (x, y);
        self.update_click_through();
    }

    /// The dog's window is much bigger than the dog (room for the bubble and hats): clicks
    /// only count on what is drawn, elsewhere they go to the windows behind it.
    fn update_click_through(&mut self) {
        let sh = &self.shell;
        // While dragging, keep the mouse.
        let over = sh.drag.is_some() || {
            let (mx, my) = mouse_px(sh.mtm);
            let (wx, wy) = (mx - sh.dog_pos.0, my - sh.dog_pos.1);
            let (_, full) = screen_at(sh.mtm, sh.dog_pos.0, sh.dog_pos.1);
            let px = self.core.scale(full.scale as f32).px as f64;
            let reach = (2.0 * px).round() as i32;
            dost::render::drawn_near(&sh.pixmap, wx.floor() as i32, wy.floor() as i32, reach)
        };
        if sh.window.ignoresMouseEvents() == over {
            sh.window.setIgnoresMouseEvents(!over);
        }
    }

    /// While hidden: shows the current bubble on its own under the menu bar icon, or hides it.
    fn paint_alone(&mut self) {
        let mtm = self.shell.mtm;
        if !self.core.bubble_alone() {
            if self.shell.alone.shown {
                self.shell.alone.shown = false;
                self.shell.alone.window.orderOut(None);
            }
            return;
        }
        let Some((ax, ay)) = status_anchor(mtm, &self.shell.status) else {
            return;
        };
        let (visible, full) = screen_at(mtm, ax, ay);
        let k = full.scale;
        let scale = self.core.scale(k as f32);
        let core = &mut self.core;
        let Some(bubble) = app::bubble_of(core.shown.as_ref(), core.waiting()) else {
            return;
        };
        let (w, h) = core.renderer.bubble_size(&bubble, scale);
        let a = &mut self.shell.alone;
        if a.pixmap.width() != w || a.pixmap.height() != h {
            a.pixmap = Pixmap::new(w, h).unwrap();
        }
        let margin = PANEL_MARGIN * k;
        let x = (ax - w as f64 / 2.0)
            .clamp(
                visible.left + margin,
                (visible.right - w as f64 - margin).max(visible.left),
            )
            .round();
        // The tail's tip just under the menu bar (the window's top edge is shadow room).
        let y = (ay - core.renderer.alone_inset(scale) as f64 + 1.0 * k).round();
        core.hits =
            core.renderer
                .draw_bubble_alone(&mut a.pixmap, (ax - x) as f32, false, &bubble, scale);
        let points = NSSize::new(w as f64 / k, h as f64 / k);
        if let Some(image) = to_image(&a.pixmap, points) {
            a.view.setImage(Some(&image));
        }
        place(mtm, &a.window, x, y, w as f64, h as f64, k);
        a.pos = (x, y, k);
        if !a.shown {
            a.shown = true;
            a.window.orderFrontRegardless();
        }
    }

    /// Draws the settings window when something changed (or its size did).
    fn paint_settings(&mut self) {
        let w = &mut self.shell.settings;
        if w.window.as_ref().is_some_and(|win| !win.isVisible()) {
            // Closed with its close button.
            w.close();
            return;
        }
        if w.panel.as_ref().is_some_and(Panel::is_editing) {
            w.redraw = true;
        }
        if !w.is_open() || !(w.redraw || w.resized()) {
            return;
        }
        w.redraw = false;
        let view = self.core.panel_view(&self.shell);
        self.shell.settings.paint(&mut self.core.renderer, &view);
    }

    /// A key for the settings window's text fields.
    fn settings_key(&mut self, key: panel::Key) {
        let view = self.core.panel_view(&self.shell);
        let Some(p) = self.shell.settings.panel.as_mut() else {
            return;
        };
        let (redraw, action) = p.key(key, &view);
        self.shell.settings.redraw |= redraw;
        if let Some(a) = action {
            self.on_settings_action(a);
        }
    }

    /// An action from the settings window: Close closes it; the rest as from the panel.
    fn on_settings_action(&mut self, action: panel::Action) {
        match action {
            panel::Action::Close => self.shell.settings.close(),
            panel::Action::OpenSettings => {}
            a => self.on_action(a),
        }
        self.shell.settings.redraw = true;
    }

    fn sync_menu(&mut self) {
        use dost::i18n::t;
        let i = &self.shell.items;
        i.show
            .set_text(t(if self.core.visible { "Hide" } else { "Show" }));
        let tr = dost::i18n::is_turkish();
        i.mute.set_text(match (self.core.settings.sound, tr) {
            (true, false) => "Mute",
            (false, false) => "Unmute",
            (true, true) => "Sesi kapat",
            (false, true) => "Sesi aç",
        });
        i.settings.set_text(t("All settings…"));
        i.quit
            .set_text(if tr { "Dost'tan çık" } else { "Quit Dost" });
        self.shell.panel.redraw = true;
        self.shell.settings.redraw = true;
    }

    fn on_menu(&mut self, id: &str) {
        match id {
            "settings" => self.shell.settings.open(self.shell.mtm),
            "show" => self.on_action(panel::Action::ToggleVisible),
            "mute" => self.core.toggle_mute(&mut self.shell),
            "quit" => self.on_action(panel::Action::Quit),
            _ => {}
        }
        self.sync_menu();
    }

    fn on_action(&mut self, action: panel::Action) {
        if action == panel::Action::TakeToy {
            // The Bone button dragged out of the panel: the toy follows the mouse from here.
            self.close_panel();
            let (mx, my) = mouse_px(self.shell.mtm);
            let lift = 12.0 * self.bounds().scale;
            self.core.take_toy(mx as f32, my as f32 + lift);
            self.shell.toy.drag = Some((0.0, -lift));
            return;
        }
        self.core.on_action(&mut self.shell, action);
        self.sync_menu();
    }

    fn toggle_panel(&mut self, anchor: Option<(f64, f64)>) {
        if self.shell.panel.is_open() {
            self.close_panel();
            return;
        }
        let at = anchor.unwrap_or_else(|| mouse_px(self.shell.mtm));
        let missed = self.core.missed;
        self.shell.panel.open(self.shell.mtm, at, anchor.is_some());
        if self.core.panel_opened()
            && let Some(p) = self.shell.panel.panel.as_mut()
        {
            p.show_missed(missed);
        }
    }

    fn paint_panel(&mut self) {
        // The caret blinks while the name is edited.
        if self
            .shell
            .panel
            .panel
            .as_ref()
            .is_some_and(Panel::is_editing)
        {
            self.shell.panel.redraw = true;
        }
        if !self.shell.panel.is_open() || !self.shell.panel.redraw {
            return;
        }
        self.shell.panel.redraw = false;
        let view = self.core.panel_view(&self.shell);
        self.shell.panel.paint(&mut self.core.renderer, &view);
    }

    /// A key for the panel's name field.
    fn panel_key(&mut self, key: panel::Key) {
        let view = self.core.panel_view(&self.shell);
        let Some(p) = self.shell.panel.panel.as_mut() else {
            return;
        };
        let (redraw, action) = p.key(key, &view);
        self.shell.panel.redraw |= redraw;
        if let Some(a) = action {
            self.on_action(a);
        }
    }

    /// Closes the panel; a name being edited is saved.
    fn close_panel(&mut self) {
        let view = self.core.panel_view(&self.shell);
        let action = self
            .shell
            .panel
            .panel
            .as_mut()
            .and_then(|p| p.finish_editing(&view));
        self.shell.panel.close();
        if let Some(a) = action {
            self.on_action(a);
        }
    }

    /// The toy's window: shown while the toy lies, flies or is dragged (not in the mouth).
    fn paint_toy(&mut self) {
        let mtm = self.shell.mtm;
        let show = self.core.toy_shown();
        let t = &mut self.shell.toy;
        if !show {
            if t.shown {
                t.shown = false;
                t.window.orderOut(None);
            }
            return;
        }
        let Some(item) = self.core.renderer.outfit().toy else {
            return;
        };
        let (_, full) = screen_at(mtm, self.core.toy.x as f64, self.core.toy.y as f64 - 10.0);
        let px = self.core.scale(full.scale as f32).px;
        let size = self.core.renderer.toy_size(item, px);
        if t.pixmap.width() != size {
            t.pixmap = Pixmap::new(size, size).unwrap();
        }
        self.core
            .renderer
            .draw_toy(&mut t.pixmap, item, self.core.toy.quarter(), px);
        let x = (self.core.toy.x as f64 - size as f64 / 2.0).round();
        let y = (self.core.toy.y as f64 - size as f64).round();
        let points = NSSize::new(size as f64 / full.scale, size as f64 / full.scale);
        if let Some(image) = to_image(&t.pixmap, points) {
            t.view.setImage(Some(&image));
        }
        place(mtm, &t.window, x, y, size as f64, size as f64, full.scale);
        if !t.shown {
            t.shown = true;
            t.window.orderFrontRegardless();
        }
    }

    /// While dragging the toy, follow the mouse; on release, throw it.
    fn toy_drag_update(&mut self) {
        let Some((dx, dy)) = self.shell.toy.drag else {
            return;
        };
        let (mx, my) = mouse_px(self.shell.mtm);
        self.core.toy.drag_to(mx as f32 - dx, my as f32 - dy);
        if NSEvent::pressedMouseButtons() & 1 == 0 {
            self.shell.toy.drag = None;
            let scale = self.bounds().scale;
            self.core.toy.release(scale);
        }
    }

    /// Mouse events on our windows, from the local event monitor. Returns true if consumed.
    fn on_mouse(&mut self, kind: NSEventType, window: Option<&NSWindow>, scroll: f64) -> bool {
        let mtm = self.shell.mtm;
        let (mx, my) = mouse_px(mtm);
        let on_dog = window.is_some_and(|w| std::ptr::eq(w, &*self.shell.window));
        let on_toy = self.shell.toy.shown
            && window.is_some_and(|w| std::ptr::eq(w, &*self.shell.toy.window));
        if on_toy {
            match kind {
                NSEventType::LeftMouseDown => {
                    self.shell.toy.drag =
                        Some((mx as f32 - self.core.toy.x, my as f32 - self.core.toy.y));
                    self.core.grab_toy();
                }
                // Right click: put it away.
                NSEventType::RightMouseUp => self.core.put_toy_away(),
                _ => {}
            }
            return matches!(
                kind,
                NSEventType::LeftMouseDown
                    | NSEventType::LeftMouseUp
                    | NSEventType::LeftMouseDragged
                    | NSEventType::RightMouseDown
                    | NSEventType::RightMouseUp
            );
        }
        let on_alone = self.shell.alone.shown
            && window.is_some_and(|w| std::ptr::eq(w, &*self.shell.alone.window));
        if on_alone {
            match kind {
                NSEventType::LeftMouseDown => self.shell.alone.pressed = true,
                // Only a whole click (down and up here) counts.
                NSEventType::LeftMouseUp if std::mem::take(&mut self.shell.alone.pressed) => {
                    let (x, y, _) = self.shell.alone.pos;
                    self.core
                        .click(&mut self.shell, (mx - x) as f32, (my - y) as f32);
                }
                _ => {}
            }
            return matches!(kind, NSEventType::LeftMouseDown | NSEventType::LeftMouseUp);
        }
        let on_status = window.is_some_and(|w| {
            self.shell
                .status
                .button(mtm)
                .and_then(|b| b.window())
                .is_some_and(|s| std::ptr::eq(w, &*s))
        });
        if on_status {
            return match kind {
                NSEventType::LeftMouseDown => {
                    let anchor = status_anchor(mtm, &self.shell.status);
                    self.toggle_panel(anchor);
                    true
                }
                // Swallow the rest of the click so the button does not track it.
                NSEventType::LeftMouseUp | NSEventType::LeftMouseDragged => true,
                _ => false,
            };
        }
        let on_settings = window.is_some_and(|w| {
            self.shell
                .settings
                .window
                .as_deref()
                .is_some_and(|p| std::ptr::eq(w, p))
        });
        if on_settings {
            return self.settings_mouse(kind, scroll);
        }
        let on_panel = window.is_some_and(|w| {
            self.shell
                .panel
                .window
                .as_deref()
                .is_some_and(|p| std::ptr::eq(w, p))
        });

        if on_panel {
            let (px, py) = self.shell.panel.local(mtm, mx, my);
            let p = &mut self.shell.panel;
            let Some(panel) = p.panel.as_mut() else {
                return false;
            };
            match kind {
                NSEventType::MouseMoved | NSEventType::LeftMouseDragged => {
                    let (redraw, action) = panel.pointer_move(px, py, &self.core.settings);
                    p.redraw |= redraw;
                    if let Some(a) = action {
                        self.on_action(a);
                    }
                }
                NSEventType::LeftMouseDown => {
                    let action = panel.pointer_down(px, py);
                    p.redraw = true;
                    if let Some(a) = action {
                        self.on_action(a);
                    }
                }
                NSEventType::LeftMouseUp => {
                    let view = self.core.panel_view(&self.shell);
                    let action = self
                        .shell
                        .panel
                        .panel
                        .as_mut()
                        .and_then(|p| p.pointer_up(px, py, &view));
                    self.shell.panel.redraw = true;
                    // Editing the name: the panel takes the keyboard.
                    if self
                        .shell
                        .panel
                        .panel
                        .as_ref()
                        .is_some_and(Panel::is_editing)
                        && let Some(w) = self.shell.panel.window.as_ref()
                    {
                        NSApplication::sharedApplication(mtm).activate();
                        w.makeKeyAndOrderFront(None);
                    }
                    if let Some(a) = action {
                        self.on_action(a);
                    }
                }
                NSEventType::ScrollWheel => {
                    let h = p.pixmap.as_ref().map_or(0, Pixmap::height);
                    panel.scroll_by(scroll as f32 / 3.0, h);
                    p.redraw = true;
                }
                _ => return false,
            }
            return true;
        }

        if on_dog {
            match kind {
                NSEventType::LeftMouseDown => {
                    self.shell.drag = Some(Drag {
                        start: NSEvent::mouseLocation(),
                        grab_dx: mx as f32 - self.core.dog.x,
                        grab_dy: my as f32 - self.core.dog.y,
                        moved: false,
                    });
                    return true;
                }
                NSEventType::RightMouseUp => {
                    self.toggle_panel(None);
                    return true;
                }
                _ => {}
            }
        }
        if kind == NSEventType::LeftMouseUp && self.shell.drag.is_some() {
            let drag = self.shell.drag.take().unwrap();
            if drag.moved {
                let ground = self.bounds().ground;
                if self.core.dog.y > ground {
                    self.core.dog.y = ground;
                }
                self.core.dog.release();
            } else {
                // Click position relative to the dog's window, physical pixels.
                let frame = self.shell.window.frame();
                let (_, full) = self.dog_screen();
                let wx = mx - frame.origin.x * full.scale;
                let wy = my - (main_height(mtm) - frame.origin.y - frame.size.height) * full.scale;
                self.core.click(&mut self.shell, wx as f32, wy as f32);
            }
            return true;
        }
        false
    }

    /// Mouse events on the settings window. Returns true if consumed (clicks in the title
    /// bar go on to AppKit, so the window can be moved and closed).
    fn settings_mouse(&mut self, kind: NSEventType, scroll: f64) -> bool {
        let Some((x, y)) = self.shell.settings.mouse() else {
            return false;
        };
        let inside = self.shell.settings.contains(x, y);
        let sw = &mut self.shell.settings;
        let Some(panel) = sw.panel.as_mut() else {
            return false;
        };
        match kind {
            NSEventType::MouseMoved | NSEventType::LeftMouseDragged => {
                let (redraw, action) = if inside {
                    panel.pointer_move(x, y, &self.core.settings)
                } else {
                    (panel.pointer_leave(), None)
                };
                sw.redraw |= redraw;
                if let Some(a) = action {
                    self.on_settings_action(a);
                }
                false
            }
            NSEventType::LeftMouseDown if inside => {
                let action = panel.pointer_down(x, y);
                sw.redraw = true;
                if let Some(a) = action {
                    self.on_settings_action(a);
                }
                false
            }
            NSEventType::LeftMouseUp => {
                let view = self.core.panel_view(&self.shell);
                let action = self
                    .shell
                    .settings
                    .panel
                    .as_mut()
                    .and_then(|p| p.pointer_up(x, y, &view));
                self.shell.settings.redraw = true;
                if let Some(a) = action {
                    self.on_settings_action(a);
                }
                false
            }
            NSEventType::ScrollWheel if inside => {
                panel.scroll_by(scroll as f32 / 3.0, 0);
                sw.redraw = true;
                true
            }
            _ => false,
        }
    }

    /// While dragging, follow the mouse (events may arrive outside our window).
    fn drag_update(&mut self) {
        let mtm = self.shell.mtm;
        let Some(drag) = self.shell.drag.as_mut() else {
            return;
        };
        let p = NSEvent::mouseLocation();
        if !drag.moved
            && ((p.x - drag.start.x).abs() > DRAG_THRESHOLD
                || (p.y - drag.start.y).abs() > DRAG_THRESHOLD)
        {
            drag.moved = true;
            self.core.dog.grab();
        }
        if drag.moved {
            let (mx, my) = mouse_px(mtm);
            let (dx, dy) = (drag.grab_dx, drag.grab_dy);
            self.core.dog.drag_to(mx as f32 - dx, my as f32 - dy);
        }
        if NSEvent::pressedMouseButtons() & 1 == 0
            && self.shell.drag.as_ref().is_some_and(|d| d.moved)
        {
            self.shell.drag = None;
            let ground = self.bounds().ground;
            if self.core.dog.y > ground {
                self.core.dog.y = ground;
            }
            self.core.dog.release();
        }
    }
}

fn install_mouse_monitor() {
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseUp
        | NSEventMask::LeftMouseDragged
        | NSEventMask::MouseMoved
        | NSEventMask::RightMouseDown
        | NSEventMask::RightMouseUp
        | NSEventMask::ScrollWheel
        | NSEventMask::KeyDown;
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        let ev = unsafe { event.as_ref() };
        let mtm = MainThreadMarker::new().unwrap();
        let kind = ev.r#type();
        let window = ev.window(mtm);
        if kind == NSEventType::KeyDown {
            // The name field takes the keys while it is edited; Esc otherwise closes the panel.
            let code = ev.keyCode();
            let text = ev.characters().map(|s| s.to_string()).unwrap_or_default();
            let handled = with_app(|a| {
                let to_settings = window.as_deref().is_some_and(|w| {
                    a.shell
                        .settings
                        .window
                        .as_deref()
                        .is_some_and(|s| std::ptr::eq(w, s))
                });
                if to_settings {
                    let cmd = ev
                        .modifierFlags()
                        .contains(objc2_app_kit::NSEventModifierFlags::Command);
                    let editing = a
                        .shell
                        .settings
                        .panel
                        .as_ref()
                        .is_some_and(Panel::is_editing);
                    let key = match code {
                        // ⌘W closes the window.
                        13 if cmd => {
                            a.shell.settings.close();
                            return true;
                        }
                        _ if !editing => return false,
                        53 => panel::Key::Escape,
                        36 | 76 => panel::Key::Enter,
                        51 => panel::Key::Backspace,
                        9 if cmd => {
                            let pb = NSPasteboard::generalPasteboard();
                            let s = pb.stringForType(unsafe { NSPasteboardTypeString });
                            panel::Key::Text(s.map(|s| s.to_string()).unwrap_or_default())
                        }
                        _ if cmd => return false,
                        _ => panel::Key::Text(text.clone()),
                    };
                    a.settings_key(key);
                    return true;
                }
                if !a.shell.panel.panel.as_ref().is_some_and(Panel::is_editing) {
                    if code == 53 {
                        a.close_panel();
                    }
                    return false;
                }
                let cmd = ev
                    .modifierFlags()
                    .contains(objc2_app_kit::NSEventModifierFlags::Command);
                let key = match code {
                    53 => panel::Key::Escape,
                    36 | 76 => panel::Key::Enter,
                    51 => panel::Key::Backspace,
                    // ⌘V: paste.
                    9 if cmd => {
                        let pb = NSPasteboard::generalPasteboard();
                        let s = pb.stringForType(unsafe { NSPasteboardTypeString });
                        panel::Key::Text(s.map(|s| s.to_string()).unwrap_or_default())
                    }
                    _ if cmd => return true,
                    _ => panel::Key::Text(text),
                };
                a.panel_key(key);
                true
            });
            return if handled == Some(true) {
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            };
        }
        if kind == NSEventType::RightMouseDown {
            let on_status = with_app(|a| {
                let status = a.shell.status.button(mtm).and_then(|b| b.window());
                window
                    .as_deref()
                    .zip(status)
                    .is_some_and(|(w, s)| std::ptr::eq(w, &*s))
            });
            if on_status == Some(true) {
                show_status_menu();
                return std::ptr::null_mut();
            }
            return event.as_ptr();
        }
        // scrollingDeltaY raises an Objective-C exception on non-scroll events, and AppKit
        // then drops the event: read it only for the scroll wheel.
        let scroll = if kind == NSEventType::ScrollWheel {
            ev.scrollingDeltaY()
        } else {
            0.0
        };
        let consumed = with_app(|a| a.on_mouse(kind, window.as_deref(), scroll)).unwrap_or(false);
        if consumed {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    std::mem::forget(monitor);

    // Clicking anywhere else closes the panel, like a menu.
    let outside = RcBlock::new(|_event: NonNull<NSEvent>| {
        with_app(App::close_panel);
    });
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
        &outside,
    );
    std::mem::forget(monitor);
}

/// The settings panel: a borderless window with a shadow showing [`Panel`].
#[derive(Default)]
struct PanelWindow {
    window: Option<Retained<NSWindow>>,
    view: Option<Retained<NSImageView>>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    /// Top-left on screen and the screen's scale.
    pos: (f64, f64, f64),
    redraw: bool,
    /// When the panel opened, for the fade-in.
    opened: Option<std::time::Instant>,
}

impl PanelWindow {
    fn is_open(&self) -> bool {
        self.window.is_some()
    }

    /// Opens at physical (`x`, `y`): just below it, or when `from_icon`, hanging from the menu
    /// bar icon there with an arrow pointing at it.
    fn open(&mut self, mtm: MainThreadMarker, (x, y): (f64, f64), from_icon: bool) {
        let (visible, full) = screen_at(mtm, x, y);
        let k = full.scale;
        let mut panel = Panel::new(k as f32);
        let margin = PANEL_MARGIN * k;
        let probe_w = (panel::WIDTH as f64 * k).ceil();
        let px = (x - probe_w / 2.0).clamp(visible.left + margin, visible.right - probe_w - margin);
        if from_icon {
            panel.set_arrow(Some((x - px) as f32));
        }
        let (w, h) = panel.size((visible.bottom - visible.top - 2.0 * margin).max(200.0) as u32);
        // From the icon the arrow takes the place of the gap.
        let gap = if from_icon { 2.0 * k } else { margin };
        let py = (visible.top + gap)
            .max(y + gap)
            .min(visible.bottom - h as f64 - margin);
        let (window, view) = make_window(mtm, true);
        place(mtm, &window, px, py, w as f64, h as f64, k);
        window.setAlphaValue(0.0);
        window.orderFrontRegardless();
        self.opened = Some(std::time::Instant::now());
        self.window = Some(window);
        self.view = Some(view);
        self.pixmap = Pixmap::new(w, h);
        self.panel = Some(panel);
        self.pos = (px, py, k);
        self.redraw = true;
    }

    fn close(&mut self) {
        if let Some(w) = self.window.take() {
            w.orderOut(None);
            w.close();
        }
        self.view = None;
        self.panel = None;
        self.pixmap = None;
    }

    /// Physical mouse position relative to the panel's top-left.
    fn local(&self, _mtm: MainThreadMarker, mx: f64, my: f64) -> (f32, f32) {
        ((mx - self.pos.0) as f32, (my - self.pos.1) as f32)
    }

    fn paint(&mut self, r: &mut Renderer, view: &panel::View) {
        let (Some(v), Some(panel), Some(pm)) = (
            self.view.as_ref(),
            self.panel.as_mut(),
            self.pixmap.as_mut(),
        ) else {
            return;
        };
        panel.draw(pm, r, view);
        let k = self.pos.2;
        if let Some(image) = to_image(
            pm,
            NSSize::new(pm.width() as f64 / k, pm.height() as f64 / k),
        ) {
            v.setImage(Some(&image));
        }
    }

    /// Fades the panel in over a short ease-out after opening; call every frame.
    fn animate(&mut self) {
        let (Some(w), Some(t)) = (self.window.as_ref(), self.opened) else {
            return;
        };
        let p = (t.elapsed().as_secs_f64() / 0.14).min(1.0);
        w.setAlphaValue(1.0 - (1.0 - p).powi(3));
        if p >= 1.0 {
            self.opened = None;
        }
    }
}

/// The big settings window: titled, closable, resizable, showing [`Panel::new_window`].
/// While it is open the app shows in the Dock and the app switcher, like a preferences
/// window; it goes back to a menu bar app when it closes.
#[derive(Default)]
struct SettingsWindow {
    window: Option<Retained<NSWindow>>,
    view: Option<Retained<NSImageView>>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    redraw: bool,
}

impl SettingsWindow {
    fn is_open(&self) -> bool {
        self.window.is_some()
    }

    fn open(&mut self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        if let Some(w) = &self.window {
            if w.isMiniaturized() {
                w.deminiaturize(None);
            }
            app.activate();
            w.makeKeyAndOrderFront(None);
            return;
        }
        let (pw, ph) = panel::WINDOW_SIZE;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(pw as f64, ph as f64));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let title = if dost::i18n::is_turkish() {
            "Dost Ayarları"
        } else {
            "Dost Settings"
        };
        window.setTitle(&NSString::from_str(title));
        window.setContentMinSize(NSSize::new(640.0, 460.0));
        window.setAcceptsMouseMovedEvents(true);
        // Centred the first time; after that where the user left it.
        window.center();
        window.setFrameAutosaveName(&NSString::from_str("DostSettings"));
        let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
        view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
        window.setContentView(Some(&view));
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.activate();
        window.makeKeyAndOrderFront(None);
        let k = window.backingScaleFactor() as f32;
        self.panel = Some(Panel::new_window(k));
        self.window = Some(window);
        self.view = Some(view);
        self.pixmap = None;
        self.redraw = true;
    }

    fn close(&mut self) {
        if let Some(w) = self.window.take() {
            w.orderOut(None);
            w.close();
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm)
                    .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }
        }
        self.view = None;
        self.panel = None;
        self.pixmap = None;
    }

    /// The content's size in physical pixels and the backing scale.
    fn content_px(&self) -> Option<(u32, u32, f64)> {
        let w = self.window.as_ref()?;
        let k = w.backingScaleFactor();
        let r = w.contentRectForFrameRect(w.frame());
        Some((
            (r.size.width * k).round().max(1.0) as u32,
            (r.size.height * k).round().max(1.0) as u32,
            k,
        ))
    }

    /// The window was resized (or moved to a screen with another scale) since the last draw.
    fn resized(&self) -> bool {
        match (self.content_px(), &self.pixmap) {
            (Some((w, h, _)), Some(pm)) => pm.width() != w || pm.height() != h,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// The mouse relative to the content's top-left, physical pixels.
    fn mouse(&self) -> Option<(f32, f32)> {
        let w = self.window.as_ref()?;
        let k = w.backingScaleFactor();
        let r = w.contentRectForFrameRect(w.frame());
        let m = NSEvent::mouseLocation();
        Some((
            ((m.x - r.origin.x) * k) as f32,
            ((r.origin.y + r.size.height - m.y) * k) as f32,
        ))
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        self.pixmap.as_ref().is_some_and(|pm| {
            x >= 0.0 && y >= 0.0 && x < pm.width() as f32 && y < pm.height() as f32
        })
    }

    fn paint(&mut self, r: &mut Renderer, view: &panel::View) {
        let Some((w, h, k)) = self.content_px() else {
            return;
        };
        if self
            .pixmap
            .as_ref()
            .is_none_or(|pm| pm.width() != w || pm.height() != h)
        {
            self.pixmap = Pixmap::new(w, h);
        }
        let (Some(v), Some(panel), Some(pm)) = (
            self.view.as_ref(),
            self.panel.as_mut(),
            self.pixmap.as_mut(),
        ) else {
            return;
        };
        panel.set_ui_scale(k as f32);
        panel.draw(pm, r, view);
        if let Some(image) = to_image(pm, NSSize::new(w as f64 / k, h as f64 / k)) {
            v.setImage(Some(&image));
        }
    }
}

/// "Open at login": a LaunchAgent that starts this executable.
mod autostart {
    use std::path::PathBuf;

    const LABEL: &str = "com.rytsh.dost";

    fn plist() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist"))
        })
    }

    pub fn enabled() -> bool {
        plist().is_some_and(|p| p.exists())
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = plist().ok_or("HOME is not set")?;
        if !on {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;");
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, xml).map_err(|e| e.to_string())
    }
}
