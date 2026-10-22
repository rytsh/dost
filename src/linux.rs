//! Linux shell (X11, and Wayland sessions through XWayland): an override-redirect, always-on-top
//! ARGB window for the dog with a click-through input shape, a StatusNotifierItem tray icon
//! (left click: settings panel, menu: the essentials) and the settings panel window.
//! Behaviour lives in [`dost::app::Core`].
//!
//! Every window is a [`Surface`]: an ARGB window showing a tiny-skia pixmap. Without a
//! compositor there is no alpha, so the drawn pixels become the window's shape instead.

use std::cell::Cell;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::properties::{WmHints, WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use dost::app::{self, Core, EDGE, Platform};
use dost::dog::Bounds;
use dost::panel::{self, Panel};
use dost::render::{Renderer, Scale};

const FRAME: Duration = Duration::from_millis(16);
/// A press that moves less than this is a click, not a drag.
const DRAG_THRESHOLD: i32 = 4;
/// Gap between the panel and the panel bar / screen edge.
const PANEL_MARGIN: i32 = 12;
/// Screens, scale and compositor are looked up again this often.
const SCREENS_EVERY: Duration = Duration::from_secs(2);
/// The lock state is asked for at most this often (it runs `loginctl`).
const LOCK_EVERY: Duration = Duration::from_secs(5);

type Res<T> = Result<T, Box<dyn std::error::Error>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_WORKAREA,
        _NET_CURRENT_DESKTOP,
        _NET_ACTIVE_WINDOW,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_STICKY,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_WINDOW_TYPE_NORMAL,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_NAME,
        _NET_WM_PID,
        _MOTIF_WM_HINTS,
        _COMPTON_SHADOW,
        UTF8_STRING,
        RESOURCE_MANAGER,
    }
}

/// The X connection and what every window needs from it.
struct X {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    /// The compositing manager's selection (`_NET_WM_CM_Sn`).
    cm: Atom,
    /// Depth and visual of our windows: 32-bit ARGB if the server has one.
    depth: u8,
    visual: Visualid,
    colormap: Colormap,
    /// Pixels go out as B, G, R, A bytes (else A, R, G, B).
    lsb: bool,
    has_shape: bool,
    /// A compositor shows our alpha; without one, windows are cut to their drawn pixels.
    composited: Cell<bool>,
    keymap: Keymap,
}

impl X {
    fn connect() -> Res<X> {
        let (conn, screen_num) = RustConnection::connect(None)?;
        let atoms = Atoms::new(&conn)?.reply()?;
        let cm = conn
            .intern_atom(false, format!("_NET_WM_CM_S{screen_num}").as_bytes())?
            .reply()?
            .atom;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let argb = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id);
        let (depth, visual) = match argb {
            Some(v) => (32, v),
            None => (screen.root_depth, screen.root_visual),
        };
        let lsb = setup.image_byte_order == ImageOrder::LSB_FIRST;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)?;
        let has_shape = conn
            .extension_information(shape::X11_EXTENSION_NAME)?
            .is_some();
        let keymap = Keymap::load(&conn)?;
        let x = X {
            conn,
            root,
            atoms,
            cm,
            depth,
            visual,
            colormap,
            lsb,
            has_shape,
            composited: Cell::new(false),
            keymap,
        };
        x.check_compositor();
        Ok(x)
    }

    fn check_compositor(&self) {
        let owner = self
            .conn
            .get_selection_owner(self.cm)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(x11rb::NONE, |r| r.owner);
        self.composited
            .set(self.depth == 32 && owner != x11rb::NONE);
    }

    fn prop32(&self, win: Window, atom: Atom) -> Vec<u32> {
        self.conn
            .get_property(false, win, atom, AtomEnum::ANY, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(Iterator::collect))
            .unwrap_or_default()
    }

    fn pointer(&self) -> (i32, i32, bool) {
        self.conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or((0, 0, false), |r| {
                (
                    r.root_x.into(),
                    r.root_y.into(),
                    r.mask.contains(KeyButMask::BUTTON1),
                )
            })
    }

    /// Monitors with their work areas, and the UI scale (Xft.dpi / 96).
    fn screens(&self) -> Screens {
        let geo = self
            .conn
            .get_geometry(self.root)
            .ok()
            .and_then(|c| c.reply().ok());
        let whole = geo.map_or(
            Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            |g| Rect {
                left: 0,
                top: 0,
                right: g.width.into(),
                bottom: g.height.into(),
            },
        );
        let mut monitors: Vec<(Rect, bool)> = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .map(|m| {
                        let (x, y) = (i32::from(m.x), i32::from(m.y));
                        let rect = Rect {
                            left: x,
                            top: y,
                            right: x + i32::from(m.width),
                            bottom: y + i32::from(m.height),
                        };
                        (rect, m.primary)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if monitors.is_empty() {
            monitors.push((whole, true));
        }
        // The primary monitor first: the dog starts there.
        monitors.sort_by_key(|(_, primary)| !primary);
        let desk = self
            .prop32(self.root, self.atoms._NET_CURRENT_DESKTOP)
            .first()
            .copied()
            .unwrap_or(0) as usize;
        let areas = self.prop32(self.root, self.atoms._NET_WORKAREA);
        let work = areas
            .get(desk * 4..desk * 4 + 4)
            .or_else(|| areas.get(0..4))
            .map(|a| Rect {
                left: a[0] as i32,
                top: a[1] as i32,
                right: (a[0] + a[2]) as i32,
                bottom: (a[1] + a[3]) as i32,
            });
        let list = monitors
            .into_iter()
            .map(|(full, _)| Monitor {
                full,
                work: work.and_then(|w| w.intersect(&full)).unwrap_or(full),
            })
            .collect();
        Screens {
            list,
            ui: self.dpi().map_or(1.0, |d| (d / 96.0).clamp(0.5, 4.0)),
        }
    }

    /// `Xft.dpi` from the root window's resources, if set.
    fn dpi(&self) -> Option<f32> {
        let r = self
            .conn
            .get_property(
                false,
                self.root,
                self.atoms.RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1 << 16,
            )
            .ok()?
            .reply()
            .ok()?;
        String::from_utf8_lossy(&r.value).lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == "Xft.dpi").then(|| v.trim().parse().ok())?
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect {
            left: self.left.max(o.left),
            top: self.top.max(o.top),
            right: self.right.min(o.right),
            bottom: self.bottom.min(o.bottom),
        };
        (r.left < r.right && r.top < r.bottom).then_some(r)
    }
}

#[derive(Clone, Copy, Debug)]
struct Monitor {
    full: Rect,
    /// Without panels and docks; `bottom` is where the dog stands.
    work: Rect,
}

struct Screens {
    /// The primary monitor first.
    list: Vec<Monitor>,
    ui: f32,
}

impl Screens {
    /// The monitor containing (x, y), or the nearest one.
    fn at(&self, x: i32, y: i32) -> Monitor {
        let dist = |r: &Rect| {
            let dx = (r.left - x).max(0).max(x - r.right + 1) as i64;
            let dy = (r.top - y).max(0).max(y - r.bottom + 1) as i64;
            dx * dx + dy * dy
        };
        self.list
            .iter()
            .min_by_key(|m| dist(&m.full))
            .copied()
            .unwrap_or(Monitor {
                full: Rect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                work: Rect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
            })
    }

    fn primary(&self) -> Monitor {
        self.list.first().copied().unwrap_or_else(|| self.at(0, 0))
    }
}

/// How a [`Surface`] behaves towards the window manager.
#[derive(Clone, Copy, PartialEq)]
enum Style {
    /// Unmanaged (override-redirect): never focused, kept on top by re-raising.
    Overlay,
    /// Managed but borderless, above other windows and off the taskbar; takes the keyboard.
    Popup,
    /// An ordinary window: decorated, resizable, in the taskbar. The window manager decides
    /// its size: call [`Surface::resized`] on `ConfigureNotify` and draw at that size.
    Window,
}

/// An ARGB window that shows a premultiplied tiny-skia pixmap.
struct Surface {
    win: Window,
    gc: Gcontext,
    style: Style,
    pos: (i32, i32),
    size: (u32, u32),
    mapped: bool,
    /// What the window shows (X byte order), for repainting on `Expose`.
    shown: Vec<u8>,
    buf: Vec<u8>,
    /// The shapes last set; `None`: the whole window.
    bounding: Option<Vec<Rect16>>,
    input: Option<Vec<Rect16>>,
    /// The smallest size the user may resize a [`Style::Window`] to.
    min_size: (u32, u32),
}

impl Surface {
    fn new(x: &X, style: Style, title: &str) -> Res<Surface> {
        let c = &x.conn;
        let a = &x.atoms;
        let win = c.generate_id()?;
        let mask = EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::LEAVE_WINDOW
            | EventMask::KEY_PRESS
            | EventMask::FOCUS_CHANGE;
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(x.colormap)
            .override_redirect(u32::from(style == Style::Overlay))
            .event_mask(mask);
        c.create_window(
            x.depth,
            win,
            x.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            x.visual,
            &aux,
        )?;
        let gc = c.generate_id()?;
        c.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))?;
        for name in [a._NET_WM_NAME, AtomEnum::WM_NAME.into()] {
            c.change_property8(
                PropMode::REPLACE,
                win,
                name,
                a.UTF8_STRING,
                title.as_bytes(),
            )?;
        }
        c.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"dost\0Dost\0",
        )?;
        c.change_property32(
            PropMode::REPLACE,
            win,
            a._NET_WM_PID,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?;
        // Compositors would otherwise shadow the whole (mostly transparent) rectangle.
        c.change_property32(
            PropMode::REPLACE,
            win,
            a._COMPTON_SHADOW,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        let kind = match style {
            Style::Overlay => a._NET_WM_WINDOW_TYPE_NOTIFICATION,
            Style::Popup => a._NET_WM_WINDOW_TYPE_UTILITY,
            Style::Window => a._NET_WM_WINDOW_TYPE_NORMAL,
        };
        c.change_property32(
            PropMode::REPLACE,
            win,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[kind],
        )?;
        if style != Style::Window {
            c.change_property32(
                PropMode::REPLACE,
                win,
                a._NET_WM_STATE,
                AtomEnum::ATOM,
                &[
                    a._NET_WM_STATE_ABOVE,
                    a._NET_WM_STATE_STICKY,
                    a._NET_WM_STATE_SKIP_TASKBAR,
                    a._NET_WM_STATE_SKIP_PAGER,
                ],
            )?;
        }
        if style == Style::Window {
            // The close button asks us instead of killing the connection.
            c.change_property32(
                PropMode::REPLACE,
                win,
                a.WM_PROTOCOLS,
                AtomEnum::ATOM,
                &[a.WM_DELETE_WINDOW],
            )?;
        }
        if style != Style::Overlay {
            let mut hints = WmHints::new();
            hints.input = Some(true);
            hints.set(c, win)?;
        }
        if style == Style::Popup {
            // flags: decorations; decorations: none.
            c.change_property32(
                PropMode::REPLACE,
                win,
                a._MOTIF_WM_HINTS,
                a._MOTIF_WM_HINTS,
                &[2, 0, 0, 0, 0],
            )?;
        }
        Ok(Surface {
            win,
            gc,
            style,
            pos: (0, 0),
            size: (1, 1),
            mapped: false,
            shown: Vec::new(),
            buf: Vec::new(),
            bounding: None,
            input: None,
            min_size: (1, 1),
        })
    }

    /// Moves and resizes the window (only if that changes anything).
    fn place(&mut self, x: &X, pos: (i32, i32), size: (u32, u32)) {
        if pos == self.pos && size == self.size {
            return;
        }
        if self.style != Style::Overlay {
            // A popup keeps exactly this place and size; a window starts there.
            let spec = match self.style {
                Style::Popup => WmSizeHintsSpecification::UserSpecified,
                _ => WmSizeHintsSpecification::ProgramSpecified,
            };
            let mut h = WmSizeHints::new();
            h.position = Some((spec, pos.0, pos.1));
            h.size = Some((spec, size.0 as i32, size.1 as i32));
            if self.style == Style::Popup {
                h.min_size = Some((size.0 as i32, size.1 as i32));
                h.max_size = h.min_size;
            } else {
                h.min_size = Some((self.min_size.0 as i32, self.min_size.1 as i32));
            }
            let _ = h.set_normal_hints(&x.conn, self.win);
        }
        let _ = x.conn.configure_window(
            self.win,
            &ConfigureWindowAux::new()
                .x(pos.0)
                .y(pos.1)
                .width(size.0.max(1))
                .height(size.1.max(1)),
        );
        if size != self.size {
            self.shown.clear();
        }
        self.pos = pos;
        self.size = size;
    }

    /// Shows `pm` (the window's size) in the window.
    fn draw(&mut self, x: &X, pm: &Pixmap) {
        self.buf.clear();
        self.buf.reserve(pm.data().len());
        for s in pm.data().as_chunks::<4>().0 {
            if x.lsb {
                self.buf.extend_from_slice(&[s[2], s[1], s[0], s[3]]);
            } else {
                self.buf.extend_from_slice(&[s[3], s[0], s[1], s[2]]);
            }
        }
        if self.style != Style::Window && !x.composited.get() {
            self.set_shape(x, SK::BOUNDING, Some(opaque_rects(pm, 128, 0)));
        } else if self.bounding.is_some() {
            self.set_shape(x, SK::BOUNDING, None);
        }
        if self.buf == self.shown {
            return;
        }
        std::mem::swap(&mut self.buf, &mut self.shown);
        self.repaint(x);
    }

    /// Puts what the window shows on screen again (after `Expose`).
    fn repaint(&self, x: &X) {
        let (w, h) = self.size;
        if !self.mapped || self.shown.len() != (w * h * 4) as usize {
            return;
        }
        let stride = w as usize * 4;
        let rows = ((x.conn.maximum_request_bytes() - 64) / stride).clamp(1, h as usize);
        for (i, chunk) in self.shown.chunks(rows * stride).enumerate() {
            let _ = x.conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.win,
                self.gc,
                w as u16,
                (chunk.len() / stride) as u16,
                0,
                (i * rows) as i16,
                0,
                x.depth,
                chunk,
            );
        }
    }

    /// The window manager gave the window this size (from `ConfigureNotify`).
    fn resized(&mut self, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            self.shown.clear();
        }
    }

    fn present(&mut self, x: &X, pm: &Pixmap, pos: (i32, i32)) {
        self.place(x, pos, (pm.width(), pm.height()));
        self.draw(x, pm);
    }

    /// Where the window takes the mouse (`None`: everywhere); elsewhere clicks go through.
    fn set_input(&mut self, x: &X, rects: Option<Vec<Rect16>>) {
        self.set_shape(x, SK::INPUT, rects);
    }

    fn set_shape(&mut self, x: &X, kind: SK, rects: Option<Vec<Rect16>>) {
        let slot = if kind == SK::INPUT {
            &mut self.input
        } else {
            &mut self.bounding
        };
        if !x.has_shape || *slot == rects {
            return;
        }
        let _ = match &rects {
            Some(r) => x.conn.shape_rectangles(
                SO::SET,
                kind,
                ClipOrdering::UNSORTED,
                self.win,
                0,
                0,
                &r.iter()
                    .map(|&(x, y, width, height)| Rectangle {
                        x,
                        y,
                        width,
                        height,
                    })
                    .collect::<Vec<_>>(),
            ),
            None => x
                .conn
                .shape_mask(SO::SET, kind, self.win, 0, 0, x11rb::NONE),
        };
        *slot = rects;
    }

    fn show(&mut self, x: &X) {
        if !self.mapped {
            self.mapped = true;
            let _ = x.conn.map_window(self.win);
            self.raise(x);
        }
    }

    fn hide(&mut self, x: &X) {
        if self.mapped {
            self.mapped = false;
            let _ = x.conn.unmap_window(self.win);
        }
    }

    fn raise(&self, x: &X) {
        if self.mapped {
            let _ = x.conn.configure_window(
                self.win,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            );
        }
    }

    /// Gives the window the keyboard (a managed window, once mapped).
    fn focus(&self, x: &X) {
        let ev = ClientMessageEvent::new(
            32,
            self.win,
            x.atoms._NET_ACTIVE_WINDOW,
            [2, x11rb::CURRENT_TIME, 0, 0, 0],
        );
        let _ = x.conn.send_event(
            false,
            x.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        );
        let _ = x
            .conn
            .set_input_focus(InputFocus::PARENT, self.win, x11rb::CURRENT_TIME);
    }

    fn destroy(self, x: &X) {
        let _ = x.conn.destroy_window(self.win);
        let _ = x.conn.free_gc(self.gc);
    }
}

/// A shape rectangle: x, y, width, height.
type Rect16 = (i16, i16, u16, u16);

/// Rectangles covering the pixels with at least `min_alpha`, each grown by `grow` pixels.
/// Equal runs on consecutive rows are merged into one rectangle.
fn opaque_rects(pm: &Pixmap, min_alpha: u8, grow: i32) -> Vec<Rect16> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let data = pm.data();
    let mut out = Vec::new();
    // Open runs: (x0, x1, first row).
    let mut open: Vec<(usize, usize, usize)> = Vec::new();
    let mut next = Vec::new();
    let close = |out: &mut Vec<Rect16>, (x0, x1, y0): (usize, usize, usize), y1: usize| {
        out.push((
            (x0 as i32 - grow) as i16,
            (y0 as i32 - grow) as i16,
            (x1 - x0) as u16 + 2 * grow as u16,
            (y1 - y0) as u16 + 2 * grow as u16,
        ));
    };
    for y in 0..=h {
        next.clear();
        if y < h {
            let row = &data[y * w * 4..(y + 1) * w * 4];
            let mut x = 0;
            while x < w {
                if row[x * 4 + 3] < min_alpha {
                    x += 1;
                    continue;
                }
                let x0 = x;
                while x < w && row[x * 4 + 3] >= min_alpha {
                    x += 1;
                }
                let y0 = open
                    .iter()
                    .find(|r| r.0 == x0 && r.1 == x)
                    .map_or(y, |r| r.2);
                next.push((x0, x, y0));
            }
        }
        for r in &open {
            if !next.iter().any(|n| n.0 == r.0 && n.1 == r.1) {
                close(&mut out, *r, y);
            }
        }
        std::mem::swap(&mut open, &mut next);
    }
    out
}

/// Keysyms of the keyboard, for typed text.
struct Keymap {
    min: u8,
    per: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(c: &RustConnection) -> Res<Keymap> {
        let s = c.setup();
        let (min, max) = (s.min_keycode, s.max_keycode);
        let r = c.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        Ok(Keymap {
            min,
            per: r.keysyms_per_keycode.max(1).into(),
            syms: r.keysyms,
        })
    }

    fn keysym(&self, code: u8, state: KeyButMask) -> u32 {
        let i = code.saturating_sub(self.min) as usize * self.per;
        let get = |k: usize| {
            if k < self.per {
                self.syms.get(i + k).copied().unwrap_or(0)
            } else {
                0
            }
        };
        let lo = get(0);
        let letter = char::from_u32(lo).is_some_and(|c| c.is_ascii_lowercase());
        let hi = match get(1) {
            0 if letter => lo - 0x20,
            0 => lo,
            s => s,
        };
        let shift = state.contains(KeyButMask::SHIFT);
        let caps = state.contains(KeyButMask::LOCK) && letter;
        if shift != caps { hi } else { lo }
    }
}

/// The character a keysym types, if any.
fn keysym_char(ks: u32) -> Option<char> {
    match ks {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(ks),
        0x0100_0000..=0x0110_ffff => char::from_u32(ks - 0x0100_0000),
        // Keypad digits, decimal point, + - * /.
        0xffb0..=0xffb9 => char::from_u32(ks - 0xffb0 + u32::from(b'0')),
        0xffae => Some('.'),
        0xffab => Some('+'),
        0xffad => Some('-'),
        0xffaa => Some('*'),
        0xffaf => Some('/'),
        _ => None,
    }
}

const XK_BACKSPACE: u32 = 0xff08;
const XK_RETURN: u32 = 0xff0d;
const XK_KP_ENTER: u32 = 0xff8d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_UP: u32 = 0xff52;
const XK_DOWN: u32 = 0xff54;
const XK_HOME: u32 = 0xff50;
const XK_END: u32 = 0xff57;

struct Drag {
    start: (i32, i32),
    /// Cursor offset from the dog's feet.
    grab_dx: f32,
    grab_dy: f32,
    moved: bool,
}

struct App {
    core: Core,
    shell: Shell,
}

/// Linux-specific state; implements [`Platform`] for the core.
struct Shell {
    x: X,
    dog: Surface,
    pixmap: Pixmap,
    drag: Option<Drag>,
    last_raise: Instant,
    screens: Screens,
    screens_at: Instant,
    tray: Option<ksni::blocking::Handle<Tray>>,
    tray_rx: Receiver<TrayMsg>,
    /// What the tray shows: (sleeping dog, unread mark, visible, sound, Turkish).
    tray_state: (bool, bool, bool, bool, &'static str, u32),
    /// Where the tray icon was last clicked, if the host told us.
    tray_anchor: Option<(i32, i32)>,
    panel: PanelWindow,
    /// The big settings window.
    settings: SettingsWindow,
    hotkeys: crate::hotkeys::Hotkeys,
    /// While hidden: the bubble on its own near the tray.
    alone: AloneBubble,
    /// The bone's window.
    toy: ToyWindow,
    sound: Option<Child>,
    /// The first sound player that worked.
    player: usize,
    lock: Cell<Option<(Instant, bool)>>,
    quit: bool,
}

/// A small window for the toy, so it can fly anywhere along the bottom of the screen.
struct ToyWindow {
    surface: Surface,
    pixmap: Pixmap,
    /// Cursor offset from the toy's bottom-centre while dragging.
    drag: Option<(f32, f32)>,
}

/// A bubble window without the dog, near the tray.
struct AloneBubble {
    surface: Surface,
    pixmap: Pixmap,
}

pub fn run() {
    let x = match X::connect() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Dost needs an X11 display (X11, or XWayland on Wayland): {e}");
            return;
        }
    };
    let surfaces = (
        Surface::new(&x, Style::Overlay, "Dost"),
        Surface::new(&x, Style::Overlay, "Dost bubble"),
        Surface::new(&x, Style::Overlay, "Dost toy"),
    );
    let (Ok(dog), Ok(alone), Ok(toy)) = surfaces else {
        eprintln!("Dost could not create its windows");
        return;
    };
    let renderer = Renderer::new(true, Vec::new());
    let (tx, tray_rx) = mpsc::channel();
    let tray = spawn_tray(tx, &renderer);

    let screens = x.screens();
    let work = screens.primary().work;
    let start_x = work.left as f32 + (work.right - work.left) as f32 * 0.75;
    let core = Core::new(renderer, start_x, work.bottom as f32);
    let shell = Shell {
        x,
        dog,
        pixmap: Pixmap::new(1, 1).unwrap(),
        drag: None,
        last_raise: Instant::now(),
        screens,
        screens_at: Instant::now(),
        tray,
        tray_rx,
        tray_state: (false, false, true, true, "", u32::MAX),
        tray_anchor: None,
        panel: PanelWindow::default(),
        settings: SettingsWindow::default(),
        hotkeys: crate::hotkeys::Hotkeys::new(),
        alone: AloneBubble {
            surface: alone,
            pixmap: Pixmap::new(1, 1).unwrap(),
        },
        toy: ToyWindow {
            surface: toy,
            pixmap: Pixmap::new(1, 1).unwrap(),
            drag: None,
        },
        sound: None,
        player: 0,
        lock: Cell::new(None),
        quit: false,
    };
    let mut app = App { core, shell };
    // Hidden last time: stays hidden.
    if app.core.on_screen() {
        app.shell.dog.show(&app.shell.x);
    }
    app.sync_menu();

    let mut next = Instant::now();
    while !app.shell.quit {
        loop {
            match app.shell.x.conn.poll_for_event() {
                Ok(Some(ev)) => app.on_event(ev),
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Dost lost the X connection: {e}");
                    app.shell.quit = true;
                    break;
                }
            }
        }
        app.tick();
        let _ = app.shell.x.conn.flush();
        next += FRAME;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    app.core.shutdown();
    if let Some(mut c) = app.shell.sound.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = std::fs::remove_file(sound_file());
    if let Some(t) = app.shell.tray.take() {
        t.shutdown().wait();
    }
}

/// Messages from the tray thread.
enum TrayMsg {
    /// Toggle the panel; where the icon was clicked, if known.
    Panel(Option<(i32, i32)>),
    /// Open the settings window.
    Settings,
    Show,
    Mute,
    Quit,
}

/// The tray icon (StatusNotifierItem). Runs on ksni's thread; talks to the loop by channel.
struct Tray {
    tx: Sender<TrayMsg>,
    icon: Vec<ksni::Icon>,
    visible: bool,
    sound: bool,
    /// The code of the app's language ("en", "tr"); set by the main loop.
    lang: &'static str,
}

impl Tray {
    fn item(label: &str, msg: fn() -> TrayMsg) -> ksni::MenuItem<Self> {
        ksni::menu::StandardItem {
            label: label.into(),
            activate: Box::new(move |t: &mut Self| {
                let _ = t.tx.send(msg());
            }),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "dost".into()
    }

    fn title(&self) -> String {
        "Dost".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Dost".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, x: i32, y: i32) {
        let at = (x != 0 || y != 0).then_some((x, y));
        let _ = self.tx.send(TrayMsg::Panel(at));
    }

    /// Right-click menu: only the essentials; everything else lives in the settings panel.
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        // Translated by the main loop (the language is set there); see `App::sync_tray`.
        let tr = |en: &'static str| dost::i18n::translate(self.lang, en);
        let mute = if self.sound { tr("Mute") } else { tr("Unmute") };
        vec![
            Self::item(tr("All settings…"), || TrayMsg::Settings),
            ksni::MenuItem::Separator,
            Self::item(tr(if self.visible { "Hide" } else { "Show" }), || {
                TrayMsg::Show
            }),
            Self::item(mute, || TrayMsg::Mute),
            ksni::MenuItem::Separator,
            Self::item(tr("Quit Dost"), || TrayMsg::Quit),
        ]
    }
}

/// The paw as ksni wants it: ARGB32, big-endian.
fn tray_icon(r: &Renderer, sleeping: bool, unread: bool) -> Vec<ksni::Icon> {
    [32, 64]
        .into_iter()
        .map(|size| ksni::Icon {
            width: size as i32,
            height: size as i32,
            data: r
                .icon_rgba(size, sleeping, unread)
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect(),
        })
        .collect()
}

/// Starts the tray icon; without a StatusNotifierItem host Dost runs on without it.
fn spawn_tray(tx: Sender<TrayMsg>, r: &Renderer) -> Option<ksni::blocking::Handle<Tray>> {
    use ksni::blocking::TrayMethods;
    let tray = Tray {
        tx,
        icon: tray_icon(r, false, false),
        visible: true,
        sound: true,
        lang: "en",
    };
    // A host that starts later (or restarts) still gets the icon.
    match tray.assume_sni_available(true).spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("Dost: no tray icon ({e}); right-click the dog for settings");
            None
        }
    }
}

fn sound_file() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("dost-{}.wav", std::process::id()))
}

/// Runs a helper program without waiting for it (a thread reaps it).
fn spawn(cmd: &mut Command) -> bool {
    match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut c) => {
            std::thread::spawn(move || c.wait());
            true
        }
        Err(_) => false,
    }
}

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Clipboard tools that take text on stdin: Wayland first, then X11.
fn copy_commands() -> Vec<(&'static str, &'static [&'static str])> {
    let mut v: Vec<(&str, &[&str])> =
        vec![("xclip", &["-selection", "clipboard"]), ("xsel", &["-bi"])];
    if wayland() {
        v.insert(0, ("wl-copy", &[]));
    }
    v
}

fn paste_text() -> Option<String> {
    let mut v: Vec<(&str, &[&str])> = vec![
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["-bo"]),
    ];
    if wayland() {
        v.insert(0, ("wl-paste", &["-n"]));
    }
    v.into_iter().find_map(|(cmd, args)| {
        let out = Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    })
}

/// The running executable; after an update replaced it, Linux reports it as "(deleted)".
fn current_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let s = exe.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(p) => p.into(),
        None => exe,
    })
}

impl Shell {
    /// Where the tray is: where its icon was clicked, else the work area's bottom-right.
    fn tray_anchor(&self) -> (i32, i32) {
        self.tray_anchor.unwrap_or_else(|| {
            let w = self.screens.primary().work;
            (w.right - 40, w.bottom)
        })
    }
}

impl Platform for Shell {
    fn play_wav(&mut self, wav: Vec<u8>) {
        const PLAYERS: [(&str, &[&str]); 3] =
            [("paplay", &[]), ("pw-play", &[]), ("aplay", &["-q"])];
        if let Some(mut c) = self.sound.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let path = sound_file();
        if std::fs::write(&path, wav).is_err() {
            return;
        }
        for (i, (cmd, args)) in PLAYERS.iter().enumerate().skip(self.player) {
            let child = Command::new(cmd)
                .args(*args)
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(c) = child {
                self.player = i;
                self.sound = Some(c);
                return;
            }
        }
    }

    fn copy_text(&mut self, text: &str) -> bool {
        use std::io::Write;
        copy_commands().into_iter().any(|(cmd, args)| {
            let Ok(mut c) = Command::new(cmd)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return false;
            };
            let ok = c
                .stdin
                .take()
                .is_some_and(|mut i| i.write_all(text.as_bytes()).is_ok());
            // The tools keep running to serve the clipboard; reap them in the background.
            std::thread::spawn(move || c.wait());
            ok
        })
    }

    fn autostart_enabled(&self) -> bool {
        autostart::enabled()
    }

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        autostart::set(on)
    }

    fn open_folder(&mut self, path: &std::path::Path) {
        spawn(Command::new("xdg-open").arg(path));
    }

    fn open_url(&mut self, url: &str) {
        spawn(Command::new("xdg-open").arg(url));
    }

    fn idle_secs(&self) -> Option<u64> {
        let r = self
            .x
            .conn
            .screensaver_query_info(self.x.root)
            .ok()?
            .reply()
            .ok()?;
        Some(u64::from(r.ms_since_user_input) / 1000)
    }

    fn fullscreen(&self) -> bool {
        let a = &self.x.atoms;
        let Some(&active) = self.x.prop32(self.x.root, a._NET_ACTIVE_WINDOW).first() else {
            return false;
        };
        active != x11rb::NONE
            && self
                .x
                .prop32(active, a._NET_WM_STATE)
                .contains(&a._NET_WM_STATE_FULLSCREEN)
    }

    fn screen_locked(&self) -> bool {
        if let Some((at, locked)) = self.lock.get()
            && at.elapsed() < LOCK_EVERY
        {
            return locked;
        }
        let session = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "auto".into());
        let locked = Command::new("loginctl")
            .args(["show-session", &session, "-p", "LockedHint", "--value"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes");
        self.lock.set(Some((Instant::now(), locked)));
        locked
    }

    fn system_notify(&mut self, title: &str, body: &str) {
        spawn(Command::new("notify-send").args(["-a", "Dost", title, body]));
    }

    fn set_dog_visible(&mut self, visible: bool) {
        if visible {
            self.dog.show(&self.x);
        } else {
            self.dog.hide(&self.x);
        }
    }

    fn close_panel(&mut self) {
        self.panel.close(&self.x);
    }

    fn quit(&mut self) {
        self.quit = true;
    }

    fn autostart_label(&self) -> &'static str {
        "Start at login"
    }

    fn open_settings_window(&mut self) {
        self.settings.open(&self.x, &self.screens);
    }

    fn open_reply(&mut self, id: u64, title: &str, question: &str) {
        if !self.panel.is_open() {
            let at = self.tray_anchor();
            self.panel.open(&self.x, &self.screens, at);
        }
        if let Some(p) = self.panel.panel.as_mut() {
            p.start_reply(id, title, question);
        }
        self.panel.redraw = true;
        // Typing goes to the panel (focused again once it is mapped).
        if let Some(s) = &self.panel.surface {
            s.focus(&self.x);
        }
    }

    fn screen_count(&self) -> usize {
        self.screens.list.len()
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
        while let Ok(msg) = self.shell.tray_rx.try_recv() {
            match msg {
                TrayMsg::Panel(at) => {
                    if at.is_some() {
                        self.shell.tray_anchor = at;
                    }
                    self.toggle_panel(at, true);
                }
                TrayMsg::Settings => {
                    let sh = &mut self.shell;
                    sh.settings.open(&sh.x, &sh.screens);
                }
                TrayMsg::Show => self.on_action(panel::Action::ToggleVisible),
                TrayMsg::Mute => self.core.toggle_mute(&mut self.shell),
                TrayMsg::Quit => self.on_action(panel::Action::Quit),
            }
            self.sync_menu();
        }
        if self.shell.screens_at.elapsed() > SCREENS_EVERY {
            self.shell.screens_at = Instant::now();
            self.shell.screens = self.shell.x.screens();
            self.shell.x.check_compositor();
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
        self.sync_tray();
        let sh = &mut self.shell;
        if sh.last_raise.elapsed() > Duration::from_secs(2) {
            // Other windows raised meanwhile; put ours back on top.
            sh.last_raise = Instant::now();
            for s in [&sh.dog, &sh.toy.surface, &sh.alone.surface] {
                s.raise(&sh.x);
            }
        }
    }

    fn sync_tray(&mut self) {
        let state = (
            self.core.dog.is_put_to_sleep(),
            self.core.missed > 0,
            self.core.visible,
            self.core.settings.sound,
            dost::i18n::code(),
            self.core.renderer.look_gen(),
        );
        if state == self.shell.tray_state {
            return;
        }
        let old = self.shell.tray_state;
        let icon = (state.0, state.1, state.5) != (old.0, old.1, old.5);
        self.shell.tray_state = state;
        let pixels = icon.then(|| tray_icon(&self.core.renderer, state.0, state.1));
        if let Some(t) = &self.shell.tray {
            t.update(|t| {
                if let Some(p) = pixels {
                    t.icon = p;
                }
                t.visible = state.2;
                t.sound = state.3;
                t.lang = state.4;
            });
        }
    }

    /// The monitor the dog is on, or the one it lives on (settings → Screen).
    fn dog_monitor(&self) -> Monitor {
        let n = self.core.settings.screen as usize;
        if n > 0
            && let Some(m) = self.shell.screens.list.get(n - 1)
        {
            return *m;
        }
        let (x, y) = self.dog_point();
        self.shell.screens.at(x, y)
    }

    fn bounds(&self) -> Bounds {
        let mon = self.dog_monitor();
        let ui = self.shell.screens.ui;
        let px = Scale::new(ui, self.core.settings.size).px as f32;
        Bounds {
            ground: app::ground_line(
                &self.core.settings,
                mon.work.bottom as f32,
                mon.full.top as f32,
                mon.full.bottom as f32,
                ui,
                px,
            ),
            left: mon.work.left as f32 + EDGE * px,
            right: mon.work.right as f32 - EDGE * px,
            scale: px,
        }
    }

    fn dog_point(&self) -> (i32, i32) {
        (self.core.dog.x as i32, self.core.dog.y as i32 - 10)
    }

    fn paint(&mut self) {
        let scale = self.core.scale(self.shell.screens.ui);
        let (px, py) = self.dog_point();
        let mon = self.shell.screens.at(px, py).full;
        let frame = self.core.dog.frame();
        let core = &mut self.core;
        let bubble = app::bubble_of(core.shown.as_ref().filter(|_| core.visible), core.waiting());
        let (w, h) = core.renderer.size(frame.anim, bubble.as_ref(), scale);
        let sh = &mut self.shell;
        if sh.pixmap.width() != w || sh.pixmap.height() != h {
            sh.pixmap = Pixmap::new(w, h).unwrap();
        }
        // The window centres the dog, clamped so the bubble stays on screen.
        let x = (core.dog.x as i32 - w as i32 / 2)
            .clamp(mon.left, (mon.right - w as i32).max(mon.left));
        let y = (core.dog.y - Renderer::ground_offset(h, scale)).round() as i32;
        core.hits = core.renderer.draw(
            &mut sh.pixmap,
            core.dog.x - x as f32,
            &frame,
            bubble.as_ref(),
            scale,
        );
        sh.dog.present(&sh.x, &sh.pixmap, (x, y));
        // The window is much bigger than the dog (room for the bubble and hats): only what
        // is drawn takes the mouse. While dragging, the press keeps the mouse anyway.
        if sh.drag.is_none() {
            let reach = 2 * scale.px as i32;
            let rects = opaque_rects(&sh.pixmap, 41, reach);
            sh.dog.set_input(&sh.x, Some(rects));
        }
    }

    /// The toy's window: shown while the toy lies, flies or is dragged (not in the mouth).
    fn paint_toy(&mut self) {
        let x = &self.shell.x;
        let t = &mut self.shell.toy;
        if !self.core.toy_shown() {
            t.surface.hide(x);
            return;
        }
        let Some(item) = self.core.renderer.outfit().toy else {
            return;
        };
        let px = self.core.scale(self.shell.screens.ui).px;
        let size = self.core.renderer.toy_size(item, px);
        if t.pixmap.width() != size {
            t.pixmap = Pixmap::new(size, size).unwrap();
        }
        self.core
            .renderer
            .draw_toy(&mut t.pixmap, item, self.core.toy.quarter(), px);
        let pos = (
            (self.core.toy.x - size as f32 / 2.0).round() as i32,
            (self.core.toy.y - size as f32).round() as i32,
        );
        t.surface.present(x, &t.pixmap, pos);
        t.surface.show(x);
    }

    fn toy_down(&mut self, (mx, my): (i32, i32)) {
        self.shell.toy.drag = Some((mx as f32 - self.core.toy.x, my as f32 - self.core.toy.y));
        self.core.grab_toy();
    }

    /// While dragging the toy, follow the mouse; when the button is up, throw it.
    fn toy_drag_update(&mut self) {
        let Some((dx, dy)) = self.shell.toy.drag else {
            return;
        };
        let (mx, my, down) = self.shell.x.pointer();
        self.core.toy.drag_to(mx as f32 - dx, my as f32 - dy);
        if !down {
            self.toy_up();
        }
    }

    fn toy_up(&mut self) {
        if self.shell.toy.drag.take().is_some() {
            let scale = self.bounds().scale;
            self.core.toy.release(scale);
        }
    }

    /// While hidden: shows the current bubble on its own next to the tray.
    fn paint_alone(&mut self) {
        if !self.core.bubble_alone() {
            self.shell.alone.surface.hide(&self.shell.x);
            return;
        }
        let anchor = self.shell.tray_anchor();
        let k = self.shell.screens.ui;
        let scale = self.core.scale(k);
        let work = self.shell.screens.at(anchor.0, anchor.1).work;
        let core = &mut self.core;
        let Some(bubble) = app::bubble_of(core.shown.as_ref(), core.waiting()) else {
            return;
        };
        let (w, h) = core.renderer.bubble_size(&bubble, scale);
        let a = &mut self.shell.alone;
        if a.pixmap.width() != w || a.pixmap.height() != h {
            a.pixmap = Pixmap::new(w, h).unwrap();
        }
        let margin = (PANEL_MARGIN as f32 * k) as i32;
        let x = (anchor.0 - w as i32 / 2).clamp(
            work.left + margin,
            (work.right - w as i32 - margin).max(work.left),
        );
        // Above a bottom panel (tail down); below a top one (tail up).
        let below = anchor.1 < (work.top + work.bottom) / 2;
        // The tail's tip right at the panel (the window's edge is shadow room).
        let inset = core.renderer.alone_inset(scale) as i32 - (k.round() as i32);
        let y = if below {
            work.top - inset
        } else {
            (work.bottom - h as i32 + inset).max(work.top)
        };
        core.hits = core.renderer.draw_bubble_alone(
            &mut a.pixmap,
            (anchor.0 - x) as f32,
            !below,
            &bubble,
            scale,
        );
        a.surface.present(&self.shell.x, &a.pixmap, (x, y));
        a.surface.show(&self.shell.x);
    }

    fn sync_menu(&mut self) {
        self.sync_tray();
        self.shell.panel.redraw = true;
        self.shell.settings.redraw = true;
    }

    fn on_action(&mut self, action: panel::Action) {
        if action == panel::Action::TakeToy {
            // The Bone button dragged out of the panel: the toy follows the mouse from here.
            self.close_panel();
            let (mx, my, _) = self.shell.x.pointer();
            let lift = 12.0 * self.bounds().scale;
            self.core.take_toy(mx as f32, my as f32 + lift);
            self.shell.toy.drag = Some((0.0, -lift));
            return;
        }
        self.core.on_action(&mut self.shell, action);
        self.sync_menu();
    }

    /// Opens the panel next to `anchor` (the tray icon), or near the cursor; closes it if open.
    fn toggle_panel(&mut self, anchor: Option<(i32, i32)>, from_tray: bool) {
        if self.shell.panel.is_open() {
            self.close_panel();
            return;
        }
        // Clicking the tray icon took the focus and so closed the panel just now: that click
        // meant "close", not "open again".
        if from_tray
            && self
                .shell
                .panel
                .lost_focus
                .is_some_and(|t| t.elapsed() < Duration::from_millis(400))
        {
            return;
        }
        let at = anchor.unwrap_or_else(|| {
            let (x, y, _) = self.shell.x.pointer();
            (x, y)
        });
        let missed = self.core.missed;
        let sh = &mut self.shell;
        sh.panel.open(&sh.x, &sh.screens, at);
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
        let sh = &mut self.shell;
        sh.panel.paint(&sh.x, &mut self.core.renderer, &view);
    }

    /// Draws the settings window when something changed (or its size did).
    fn paint_settings(&mut self) {
        let w = &mut self.shell.settings;
        if w.panel.as_ref().is_some_and(Panel::is_editing) {
            w.redraw = true;
        }
        if !w.is_open() || !(w.redraw || w.resized()) {
            return;
        }
        w.redraw = false;
        let view = self.core.panel_view(&self.shell);
        let sh = &mut self.shell;
        sh.settings.paint(&sh.x, &mut self.core.renderer, &view);
    }

    /// An action from the settings window: Close closes it; the rest as from the panel.
    fn on_settings_action(&mut self, action: panel::Action) {
        match action {
            panel::Action::Close => self.shell.settings.close(&self.shell.x),
            panel::Action::OpenSettings => {}
            a => self.on_action(a),
        }
        self.shell.settings.redraw = true;
    }

    /// A key for the panel's fields.
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
        self.shell.panel.close(&self.shell.x);
        if let Some(a) = action {
            self.on_action(a);
        }
    }

    fn mouse_down(&mut self, (mx, my): (i32, i32)) {
        self.shell.drag = Some(Drag {
            start: (mx, my),
            grab_dx: mx as f32 - self.core.dog.x,
            grab_dy: my as f32 - self.core.dog.y,
            moved: false,
        });
    }

    /// While dragging, follow the mouse.
    fn drag_update(&mut self) {
        let Some(drag) = self.shell.drag.as_mut() else {
            return;
        };
        let (mx, my, down) = self.shell.x.pointer();
        if !drag.moved
            && ((mx - drag.start.0).abs() > DRAG_THRESHOLD
                || (my - drag.start.1).abs() > DRAG_THRESHOLD)
        {
            drag.moved = true;
            self.core.dog.grab();
        }
        if drag.moved {
            self.core
                .dog
                .drag_to(mx as f32 - drag.grab_dx, my as f32 - drag.grab_dy);
            // The release went elsewhere: drop it here.
            if !down {
                self.mouse_up((mx, my));
            }
        }
    }

    fn mouse_up(&mut self, (mx, my): (i32, i32)) {
        let Some(drag) = self.shell.drag.take() else {
            return;
        };
        if drag.moved {
            // Falls onto the ground of the screen it was dropped on.
            let ground = self.bounds().ground;
            if self.core.dog.y > ground {
                self.core.dog.y = ground;
            }
            self.core.dog.release();
            return;
        }
        let (wx, wy) = self.shell.dog.pos;
        self.core
            .click(&mut self.shell, (mx - wx) as f32, (my - wy) as f32);
    }

    fn on_event(&mut self, ev: Event) {
        let sh = &self.shell;
        let panel_win = sh.panel.surface.as_ref().map(|s| s.win);
        let settings_win = sh.settings.surface.as_ref().map(|s| s.win);
        if settings_win.is_some() && settings_win == event_window(&ev) {
            self.settings_event(ev);
            return;
        }
        match ev {
            Event::Expose(e) if e.count == 0 => {
                let sh = &self.shell;
                for s in [&sh.dog, &sh.toy.surface, &sh.alone.surface]
                    .into_iter()
                    .chain(sh.panel.surface.as_ref())
                    .chain(sh.settings.surface.as_ref())
                {
                    if s.win == e.window {
                        s.repaint(&sh.x);
                    }
                }
            }
            Event::MapNotify(e) if Some(e.window) == panel_win => {
                if let Some(s) = &sh.panel.surface {
                    s.focus(&sh.x);
                }
            }
            Event::MappingNotify(_) => {
                if let Ok(k) = Keymap::load(&self.shell.x.conn) {
                    self.shell.x.keymap = k;
                }
            }
            Event::ButtonPress(e) => {
                let at = (i32::from(e.root_x), i32::from(e.root_y));
                if Some(e.event) == panel_win {
                    self.panel_button(e.detail, true, e.event_x, e.event_y);
                } else if e.detail == 1 && e.event == sh.toy.surface.win {
                    self.toy_down(at);
                } else if e.detail == 1 && e.event == sh.dog.win {
                    self.mouse_down(at);
                }
            }
            Event::ButtonRelease(e) => {
                let at = (i32::from(e.root_x), i32::from(e.root_y));
                if Some(e.event) == panel_win {
                    self.panel_button(e.detail, false, e.event_x, e.event_y);
                } else if e.event == sh.toy.surface.win {
                    match e.detail {
                        1 => self.toy_up(),
                        // Right click on the toy: put it away.
                        3 => self.core.put_toy_away(),
                        _ => {}
                    }
                } else if e.event == sh.alone.surface.win {
                    let (w, h) = sh.alone.surface.size;
                    let (x, y) = (i32::from(e.event_x), i32::from(e.event_y));
                    if e.detail == 1 && x >= 0 && y >= 0 && x < w as i32 && y < h as i32 {
                        self.core.click(&mut self.shell, x as f32, y as f32);
                    }
                } else if e.event == sh.dog.win {
                    match e.detail {
                        1 => self.mouse_up(at),
                        3 => self.toggle_panel(None, false),
                        _ => {}
                    }
                }
            }
            Event::MotionNotify(e) if Some(e.event) == panel_win => {
                let settings = self.core.settings.clone();
                let (x, y) = (f32::from(e.event_x), f32::from(e.event_y));
                let (redraw, action) = self
                    .shell
                    .panel
                    .panel
                    .as_mut()
                    .map_or((false, None), |p| p.pointer_move(x, y, &settings));
                self.shell.panel.redraw |= redraw;
                if let Some(a) = action {
                    self.on_action(a);
                }
            }
            Event::LeaveNotify(e) if Some(e.event) == panel_win => {
                if self
                    .shell
                    .panel
                    .panel
                    .as_mut()
                    .is_some_and(Panel::pointer_leave)
                {
                    self.shell.panel.redraw = true;
                }
            }
            Event::KeyPress(e) if Some(e.event) == panel_win => {
                let ks = self.shell.x.keymap.keysym(e.detail, e.state);
                self.panel_keysym(ks, e.state);
            }
            Event::FocusIn(e) if Some(e.event) == panel_win => {
                self.shell.panel.focused = true;
            }
            Event::FocusOut(e)
                if Some(e.event) == panel_win
                    && e.mode == NotifyMode::NORMAL
                    && e.detail != NotifyDetail::INFERIOR
                    && self.shell.panel.focused =>
            {
                // Clicked elsewhere: close like a flyout.
                self.close_panel();
                self.shell.panel.lost_focus = Some(Instant::now());
            }
            _ => {}
        }
    }

    fn panel_button(&mut self, button: u8, down: bool, x: i16, y: i16) {
        let (x, y) = (f32::from(x), f32::from(y));
        match (button, down) {
            (1, true) => {
                let action = self
                    .shell
                    .panel
                    .panel
                    .as_mut()
                    .and_then(|p| p.pointer_down(x, y));
                self.shell.panel.redraw = true;
                if let Some(a) = action {
                    self.on_action(a);
                }
            }
            (1, false) => {
                let view = self.core.panel_view(&self.shell);
                let action = self
                    .shell
                    .panel
                    .panel
                    .as_mut()
                    .and_then(|p| p.pointer_up(x, y, &view));
                self.shell.panel.redraw = true;
                if let Some(a) = action {
                    self.on_action(a);
                }
            }
            // The wheel: 4 up, 5 down.
            (4 | 5, true) => {
                let h = self.shell.panel.height();
                if let Some(p) = self.shell.panel.panel.as_mut() {
                    p.scroll_by(if button == 4 { 1.0 } else { -1.0 }, h);
                    self.shell.panel.redraw = true;
                }
            }
            _ => {}
        }
    }

    fn panel_keysym(&mut self, ks: u32, state: KeyButMask) {
        let editing = self
            .shell
            .panel
            .panel
            .as_ref()
            .is_some_and(Panel::is_editing);
        let ctrl = state.contains(KeyButMask::CONTROL);
        match ks {
            // Esc: cancels editing, or closes the panel.
            XK_ESCAPE if editing => self.panel_key(panel::Key::Escape),
            XK_ESCAPE => self.close_panel(),
            XK_BACKSPACE => self.panel_key(panel::Key::Backspace),
            XK_RETURN | XK_KP_ENTER => self.panel_key(panel::Key::Enter),
            XK_UP => self.panel_key(panel::Key::Up),
            XK_DOWN => self.panel_key(panel::Key::Down),
            XK_HOME => self.panel_key(panel::Key::Home),
            XK_END => self.panel_key(panel::Key::End),
            0x76 | 0x56 if ctrl => {
                if let Some(t) = paste_text() {
                    self.panel_key(panel::Key::Text(t));
                }
            }
            _ if ctrl => {}
            _ => {
                if let Some(c) = keysym_char(ks) {
                    self.panel_key(panel::Key::Text(c.to_string()));
                }
            }
        }
    }
}

/// The window an event is for, for the events the settings window handles.
fn event_window(ev: &Event) -> Option<Window> {
    Some(match ev {
        Event::Expose(e) => e.window,
        Event::ConfigureNotify(e) => e.window,
        Event::MapNotify(e) => e.window,
        Event::ClientMessage(e) => e.window,
        Event::DestroyNotify(e) => e.window,
        Event::ButtonPress(e) | Event::ButtonRelease(e) => e.event,
        Event::MotionNotify(e) => e.event,
        Event::LeaveNotify(e) => e.event,
        Event::KeyPress(e) => e.event,
        _ => return None,
    })
}

impl App {
    /// Events of the settings window. It stays open when it loses focus.
    fn settings_event(&mut self, ev: Event) {
        let atoms = self.shell.x.atoms;
        match ev {
            Event::Expose(e) if e.count == 0 => {
                if let Some(s) = &self.shell.settings.surface {
                    s.repaint(&self.shell.x);
                }
            }
            Event::MapNotify(_) => {
                if let Some(s) = &self.shell.settings.surface {
                    s.focus(&self.shell.x);
                }
            }
            Event::ConfigureNotify(e) => {
                if let Some(s) = self.shell.settings.surface.as_mut() {
                    s.resized((e.width.max(1).into(), e.height.max(1).into()));
                }
            }
            Event::ClientMessage(e)
                if e.type_ == atoms.WM_PROTOCOLS
                    && e.format == 32
                    && e.data.as_data32()[0] == atoms.WM_DELETE_WINDOW =>
            {
                self.settings_close();
            }
            Event::DestroyNotify(_) => {
                self.shell.settings.surface = None;
                self.shell.settings.close(&self.shell.x);
            }
            Event::ButtonPress(e) | Event::ButtonRelease(e) => {
                let down = matches!(ev, Event::ButtonPress(_));
                self.settings_button(e.detail, down, e.event_x, e.event_y);
            }
            Event::MotionNotify(e) => {
                let settings = self.core.settings.clone();
                let (x, y) = (f32::from(e.event_x), f32::from(e.event_y));
                let sw = &mut self.shell.settings;
                let (redraw, action) = sw
                    .panel
                    .as_mut()
                    .map_or((false, None), |p| p.pointer_move(x, y, &settings));
                sw.redraw |= redraw;
                if let Some(a) = action {
                    self.on_settings_action(a);
                }
            }
            Event::LeaveNotify(_) => {
                let sw = &mut self.shell.settings;
                if sw.panel.as_mut().is_some_and(Panel::pointer_leave) {
                    sw.redraw = true;
                }
            }
            Event::KeyPress(e) => {
                let ks = self.shell.x.keymap.keysym(e.detail, e.state);
                self.settings_keysym(ks, e.state);
            }
            _ => {}
        }
    }

    /// Closes the settings window; a field being edited is saved.
    fn settings_close(&mut self) {
        let view = self.core.panel_view(&self.shell);
        let action = self
            .shell
            .settings
            .panel
            .as_mut()
            .and_then(|p| p.finish_editing(&view));
        self.shell.settings.close(&self.shell.x);
        if let Some(a) = action {
            self.on_action(a);
        }
    }

    fn settings_button(&mut self, button: u8, down: bool, x: i16, y: i16) {
        let (x, y) = (f32::from(x), f32::from(y));
        match (button, down) {
            (1, true) => {
                let sw = &mut self.shell.settings;
                let action = sw.panel.as_mut().and_then(|p| p.pointer_down(x, y));
                sw.redraw = true;
                if let Some(a) = action {
                    self.on_settings_action(a);
                }
            }
            (1, false) => {
                let view = self.core.panel_view(&self.shell);
                let sw = &mut self.shell.settings;
                let action = sw.panel.as_mut().and_then(|p| p.pointer_up(x, y, &view));
                sw.redraw = true;
                if let Some(a) = action {
                    self.on_settings_action(a);
                }
            }
            (4 | 5, true) => {
                let sw = &mut self.shell.settings;
                if let Some(p) = sw.panel.as_mut() {
                    p.scroll_by(if button == 4 { 1.0 } else { -1.0 }, 0);
                    sw.redraw = true;
                }
            }
            _ => {}
        }
    }

    fn settings_keysym(&mut self, ks: u32, state: KeyButMask) {
        let editing = self
            .shell
            .settings
            .panel
            .as_ref()
            .is_some_and(Panel::is_editing);
        let ctrl = state.contains(KeyButMask::CONTROL);
        let key = match ks {
            // Ctrl+W closes the window.
            0x77 | 0x57 if ctrl => return self.settings_close(),
            _ if !editing => return,
            XK_ESCAPE => panel::Key::Escape,
            XK_BACKSPACE => panel::Key::Backspace,
            XK_RETURN | XK_KP_ENTER => panel::Key::Enter,
            XK_UP => panel::Key::Up,
            XK_DOWN => panel::Key::Down,
            XK_HOME => panel::Key::Home,
            XK_END => panel::Key::End,
            0x76 | 0x56 if ctrl => match paste_text() {
                Some(t) => panel::Key::Text(t),
                None => return,
            },
            _ if ctrl => return,
            _ => match keysym_char(ks) {
                Some(c) => panel::Key::Text(c.to_string()),
                None => return,
            },
        };
        let view = self.core.panel_view(&self.shell);
        let sw = &mut self.shell.settings;
        let Some(p) = sw.panel.as_mut() else {
            return;
        };
        let (redraw, action) = p.key(key, &view);
        sw.redraw |= redraw;
        if let Some(a) = action {
            self.on_settings_action(a);
        }
    }
}

/// The big settings window: decorated, resizable and in the taskbar, showing
/// [`Panel::new_window`]. Unlike the popover it stays open when it loses focus.
#[derive(Default)]
struct SettingsWindow {
    surface: Option<Surface>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    redraw: bool,
}

/// The settings window's smallest size, logical pixels.
const SETTINGS_MIN: (f32, f32) = (640.0, 460.0);

impl SettingsWindow {
    fn is_open(&self) -> bool {
        self.surface.is_some()
    }

    /// Opens the window centred on the primary monitor, or brings it to the front.
    fn open(&mut self, x: &X, screens: &Screens) {
        if let Some(s) = &self.surface {
            s.raise(x);
            s.focus(x);
            return;
        }
        let ui = screens.ui;
        let title = dost::i18n::t("Dost Settings");
        let Ok(mut surface) = Surface::new(x, Style::Window, title) else {
            return;
        };
        let work = screens.primary().work;
        let (pw, ph) = panel::WINDOW_SIZE;
        let w = ((pw * ui) as i32).min(work.right - work.left).max(1);
        let h = ((ph * ui) as i32).min(work.bottom - work.top).max(1);
        surface.min_size = (
            ((SETTINGS_MIN.0 * ui) as u32).min(w as u32),
            ((SETTINGS_MIN.1 * ui) as u32).min(h as u32),
        );
        let pos = (
            work.left + (work.right - work.left - w) / 2,
            work.top + (work.bottom - work.top - h) / 2,
        );
        surface.place(x, pos, (w as u32, h as u32));
        surface.show(x);
        self.surface = Some(surface);
        self.panel = Some(Panel::new_window(ui));
        self.pixmap = None;
        self.redraw = true;
    }

    fn close(&mut self, x: &X) {
        if let Some(s) = self.surface.take() {
            s.destroy(x);
        }
        self.panel = None;
        self.pixmap = None;
    }

    /// The window's size changed since the last draw.
    fn resized(&self) -> bool {
        match (&self.surface, &self.pixmap) {
            (Some(s), Some(pm)) => (pm.width(), pm.height()) != s.size,
            (Some(_), None) => true,
            _ => false,
        }
    }

    fn paint(&mut self, x: &X, r: &mut Renderer, view: &panel::View) {
        let (Some(s), Some(panel)) = (self.surface.as_mut(), self.panel.as_mut()) else {
            return;
        };
        let (w, h) = s.size;
        if self
            .pixmap
            .as_ref()
            .is_none_or(|pm| (pm.width(), pm.height()) != (w, h))
        {
            self.pixmap = Pixmap::new(w, h);
        }
        let Some(pm) = self.pixmap.as_mut() else {
            return;
        };
        panel.draw(pm, r, view);
        s.draw(x, pm);
    }
}

/// Settings panel window: a borderless popup showing [`Panel`]. It takes the keyboard and
/// closes when it loses focus, like a flyout.
#[derive(Default)]
struct PanelWindow {
    surface: Option<Surface>,
    panel: Option<Panel>,
    pixmap: Option<Pixmap>,
    pos: (i32, i32),
    redraw: bool,
    /// It had the focus (so losing it means the user went elsewhere).
    focused: bool,
    /// When it last closed because the focus went elsewhere.
    lost_focus: Option<Instant>,
}

impl PanelWindow {
    fn is_open(&self) -> bool {
        self.surface.is_some()
    }

    fn open(&mut self, x: &X, screens: &Screens, anchor: (i32, i32)) {
        let ui = screens.ui;
        let mut panel = Panel::new(ui);
        let work = screens.at(anchor.0, anchor.1).work;
        let margin = (PANEL_MARGIN as f32 * ui) as i32;
        let (w, h) = panel.size((work.bottom - work.top - 2 * margin).max(200) as u32);
        // Above the tray icon when the panel bar is at the bottom; clamp into the work area.
        let px = (anchor.0 - w as i32 / 2).clamp(
            work.left + margin,
            (work.right - w as i32 - margin).max(work.left),
        );
        let py = if anchor.1 >= work.bottom - margin {
            work.bottom - h as i32 - margin
        } else {
            (anchor.1 + margin).clamp(
                work.top + margin,
                (work.bottom - h as i32 - margin).max(work.top),
            )
        };
        panel.set_ui_scale(ui);
        let Ok(mut surface) = Surface::new(x, Style::Popup, "Dost") else {
            return;
        };
        surface.place(x, (px, py), (w, h));
        self.surface = Some(surface);
        self.pixmap = Pixmap::new(w, h);
        self.panel = Some(panel);
        self.pos = (px, py);
        self.redraw = true;
        self.focused = false;
    }

    fn close(&mut self, x: &X) {
        if let Some(s) = self.surface.take() {
            s.destroy(x);
        }
        self.panel = None;
        self.pixmap = None;
    }

    fn paint(&mut self, x: &X, r: &mut Renderer, view: &panel::View) {
        let (Some(s), Some(panel), Some(pm)) = (
            self.surface.as_mut(),
            self.panel.as_mut(),
            self.pixmap.as_mut(),
        ) else {
            return;
        };
        panel.draw(pm, r, view);
        s.present(x, pm, self.pos);
        s.show(x);
    }

    fn height(&self) -> u32 {
        self.pixmap.as_ref().map_or(0, Pixmap::height)
    }
}

/// "Start at login": an XDG autostart entry that starts this executable.
mod autostart {
    use std::path::PathBuf;

    fn entry() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("autostart").join("dost.desktop"))
    }

    pub fn enabled() -> bool {
        entry().is_some_and(|p| p.exists())
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = entry().ok_or("HOME is not set")?;
        if !on {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        let exe = super::current_exe().ok_or("cannot find the dost executable")?;
        // Desktop entry quoting: inside double quotes, escape " ` $ and \.
        let mut quoted = String::from("\"");
        for c in exe.to_string_lossy().chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Dost\nComment=A pixel-art desktop dog\n\
             Exec={quoted}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_cover_drawn_pixels() {
        let mut pm = Pixmap::new(6, 4).unwrap();
        let px = tiny_skia::PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
        for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2), (4, 3)] {
            pm.pixels_mut()[y * 6 + x] = px;
        }
        assert_eq!(opaque_rects(&pm, 1, 0), vec![(1, 1, 2, 2), (4, 3, 1, 1)]);
        assert_eq!(opaque_rects(&pm, 1, 1)[0], (0, 0, 4, 4));
    }

    #[test]
    fn keysyms_to_text() {
        assert_eq!(keysym_char(0x61), Some('a'));
        assert_eq!(keysym_char(0xe9), Some('é'));
        assert_eq!(keysym_char(0x0100_20ac), Some('€'));
        assert_eq!(keysym_char(0xffb7), Some('7'));
        assert_eq!(keysym_char(XK_RETURN), None);
    }
}
