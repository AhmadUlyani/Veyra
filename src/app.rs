use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Power::POWERBROADCAST_SETTING;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::events::{self, Event};
use crate::gfx::{Al, Gfx, BLACK, C, WHITE};
use crate::lyrics::{self, Line};
use crate::media::{MediaCmd, MediaState};
use crate::store::{self, Data, Task};
use crate::util::*;
use crate::{clip, privacy, qr, shelf, shots, sound, system, theme, thumbs, tray, update};

// ------------------------------------------------------------------ style

const T2: C = C(1.0, 1.0, 1.0, 0.62);
const T3: C = C(1.0, 1.0, 1.0, 0.38);
/// White that stays white in every theme (exact white is swapped for the theme's text color).
const PURE_WHITE: C = C(0.9999, 0.9999, 0.9999, 1.0);
const SURF: C = C(1.0, 1.0, 1.0, 0.08);
const SURF_H: C = C(1.0, 1.0, 1.0, 0.16);
const FOCUS_C: C = C::hex(0xFF7A59);
const BREAK_C: C = C::hex(0x4CD787);
const WATER_C: C = C::hex(0x4DB8FF);
const TASK_C: C = C::hex(0xFFC53D);
const SHELF_C: C = C::hex(0xB18CFF);
const RED: C = C::hex(0xFF5C5C);
const GREEN: C = C::hex(0x4CD787);
const MEDIA_C: C = C::hex(0xF2F2F7);
const CLIP_C: C = C::hex(0x5AC8FA);
const SETTINGS_C: C = C::hex(0xAEAEB2);

/// Quick colors on the Colors page: deep tones for the backgrounds, bright ones for the rest.
const QUICK_BG: [u32; 8] = [0x000000, 0x1C1C1E, 0x2B2D31, 0x0F1B3D, 0x132A5E, 0x2A1740, 0x0F2A22, 0xF4EFE6];
const QUICK_ACCENT: [u32; 8] = [0xFF7A59, 0xFFC53D, 0x4CD787, 0x5EC8FF, 0x6E9BFF, 0xB18CFF, 0xFF6FAE, 0xE8EAED];

/// Text colors that read on dark backgrounds, and on light ones.
const QUICK_TEXT: [u32; 8] = [0xFFFFFF, 0xE8EAED, 0xC9CED6, 0xF4EFE6, 0xA9C4FF, 0xFFD6A5, 0xB7F0C8, 0xFFB3C7];
const QUICK_TEXT_LIGHT: [u32; 8] = [0x111111, 0x3B342C, 0x5A2E1A, 0x7A1F3D, 0x1F3A6E, 0x1F4D36, 0x4B2A6B, 0x8A4A0F];

/// Colors page: the "All icons" choice (0-2 are island parts, 3-9 the features).
const COL_ICONS: u8 = 10;
/// Colors page: the text (belongs with the island parts).
const COL_TEXT: u8 = 11;

/// A ready-made look on the Colors page: the background (one color, or a gradient from the
/// bottom of the island to the top) and one color family that goes with it: `main` for the
/// highlights, every feature (tab icons, timer, buttons, notices) and the music bars, `soft` (a
/// gentler shade of it) for every other icon, the break timer and a lyric line under the mouse.
/// "auto" = the built-in colors. Light looks use darker shades so everything stays readable.
/// `extra`: settings of the look's own, applied last (a darker button with white text, ...).
struct Look {
    name: &'static str,
    bg: &'static str,
    main: &'static str,
    soft: &'static str,
    extra: &'static [(&'static str, &'static str)],
}

const PRESETS: [Look; 8] = [
    Look { name: "Black", bg: "#000000", main: "auto", soft: "auto", extra: &[] },
    Look { name: "Graphite", bg: "#2B2D31", main: "#C9CED6", soft: "#A9AFB9", extra: &[] },
    Look { name: "Midnight", bg: "#0F1B3D", main: "#6E9BFF", soft: "#9DB9FF", extra: &[] },
    Look { name: "Plum", bg: "#2A1740", main: "#C792EA", soft: "#DDB8F2", extra: &[] },
    Look { name: "Forest", bg: "#0F2A22", main: "#5FD08A", soft: "#93DDB0", extra: &[] },
    // night blue at the bottom fading to black at the top, open or closed
    Look {
        name: "Night",
        bg: "#132A5E 0, #0A1430 0.4, #030509 1",
        main: "#8FB0FF",
        soft: "#A9C4FF",
        extra: &[
            ("button_accent", "#1F4AA8"),
            ("button_accent_text", "#FFFFFF"),
            ("audio_bar", "#6F9BFF"),
            ("spectrum", "#6F9BFF"),
            ("lyrics_hover", "#9DB9FF"),
            ("text_contrast", "high"),
        ],
    },
    Look { name: "Blush", bg: "#F07A8A 0, #F2B5BC 0.45, #F2DCE0 0.75", main: "#B0245A", soft: "#8A3550", extra: &[] },
    Look { name: "Cream", bg: "#F4EFE6", main: "#A2531F", soft: "#7E5634", extra: &[] },
];

/// theme.conf settings of a ready-made look: background (the same when open) and its color
/// family everywhere; text picked automatically so it stays readable.
fn preset_keys(l: &Look) -> Vec<(&'static str, String)> {
    let mut kv = if l.bg.contains(',') {
        vec![("mode", "gradient".to_string()), ("gradient", l.bg.to_string())]
    } else {
        vec![("mode", "solid".to_string()), ("solid", l.bg.to_string())]
    };
    // everything else that could clash with the look goes back to automatic
    for k in [
        "text",
        "text_contrast",
        "expanded_text",
        "expanded_text_contrast",
        "icon_active",
        "button_accent_text",
        "button",
        "button_hover",
        "button_text",
        "button_active",
    ] {
        kv.push((k, "auto".into()));
    }
    kv.push(("expanded_mode", "same".into()));
    for k in ["button_accent", "audio_bar", "spectrum"] {
        kv.push((k, l.main.into()));
    }
    for k in theme::FEATURE_KEYS {
        kv.push((k, l.main.into()));
    }
    for k in ["icon", "timer_break", "lyrics_hover"] {
        kv.push((k, l.soft.into()));
    }
    for (k, v) in l.extra {
        kv.push((k, v.to_string()));
    }
    kv
}

/// The background stops of a ready-made look.
fn preset_bg(bg: &str) -> Vec<(f32, C)> {
    let mode = if bg.contains(',') { "gradient" } else { "solid" };
    theme::parse(&format!("mode = {mode}\nsolid = {bg}\ngradient = {bg}")).bg
}
/// the shortcut that mutes / unmutes every microphone: the user's own (Settings), else the first
/// one free of these (NVIDIA's overlay, for one, can hold Ctrl+Alt+M)
const HOTKEY_MIC: i32 = 1;
const MIC_SHORTCUTS: [(u32, u32); 3] = [
    (MOD_CONTROL.0 | MOD_ALT.0, 'M' as u32),
    (MOD_CONTROL.0 | MOD_ALT.0, 'K' as u32),
    (MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0, 'M' as u32),
];

// shelf tiles (DIP); the shelf page is 500 wide with 18 px padding on each side
const SHELF_TW: f32 = 80.0;
const SHELF_TH: f32 = 96.0;
const SHELF_GAP: f32 = 9.0;
const SHELF_BODY_W: f32 = 500.0 - 36.0;

/// QR notice: room for the link before it is cut short, space between its buttons
const QR_TEXT_MAX: f32 = 250.0;
const QR_BTN_GAP: f32 = 6.0;

/// The buttons a QR code gets: (label, 0 open / 1 copy), the main one first.
fn qr_buttons(code: &str) -> Vec<(&'static str, u8)> {
    match qr::kind(code) {
        qr::Kind::Link(_) => vec![("Open", 0), ("Copy", 1)],
        qr::Kind::Wifi(_, Some(_)) => vec![("Copy password", 1)],
        qr::Kind::Wifi(_, None) => vec![("Copy name", 1)],
        qr::Kind::Text(_) => vec![("Copy", 1)],
    }
}

fn qr_btn_w(g: &Gfx, label: &str) -> f32 {
    // wide enough for "Copied" too, so the button keeps its size when it says so
    g.measure(label, 11.5, 700).max(g.measure("Copied", 11.5, 600)) + 22.0
}

fn qr_btns_w(g: &Gfx, code: &str) -> f32 {
    let b = qr_buttons(code);
    b.iter().map(|(l, _)| qr_btn_w(g, l)).sum::<f32>() + QR_BTN_GAP * (b.len().saturating_sub(1)) as f32
}

/// (what it is, one line of what it says): the link without https://, the Wi-Fi network
/// (never its password), the text on one line.
fn qr_label(code: &str) -> (&'static str, String) {
    match qr::kind(code) {
        qr::Kind::Link(u) => {
            let low = u.to_ascii_lowercase();
            let cut = if low.starts_with("https://") { 8 } else if low.starts_with("http://") { 7 } else { 0 };
            ("link", u[cut..].trim_end_matches('/').to_string())
        }
        qr::Kind::Wifi(s, Some(_)) => ("Wi-Fi", s),
        qr::Kind::Wifi(s, None) => ("Wi-Fi", format!("{s} · open network")),
        qr::Kind::Text(t) => ("text", clip::preview(&t)),
    }
}

/// How many shelf tiles fit side by side in a body `w` wide.
fn shelf_per(w: f32) -> usize {
    (((w + SHELF_GAP) / (SHELF_TW + SHELF_GAP)).floor() as usize).max(1)
}

/// Width of the concave fillet joining the notch to the top screen edge (DIP).
const SHOULDER: f32 = 9.0;

mod ic {
    pub const MUSIC: char = '\u{E8D6}';
    pub const PLAY: char = '\u{E768}';
    pub const PAUSE: char = '\u{E769}';
    pub const PREV: char = '\u{E892}';
    pub const NEXT: char = '\u{E893}';
    pub const LYRICS: char = '\u{E8BD}';
    pub const TIMER: char = '\u{E916}';
    pub const FOLDER: char = '\u{E8B7}';
    pub const TASKS: char = '\u{E9D5}';
    pub const VOL: char = '\u{E767}';
    pub const MUTE: char = '\u{E74F}';
    pub const BRIGHT: char = '\u{E706}';
    pub const WIFI: char = '\u{E701}';
    pub const ETHERNET: char = '\u{E839}';
    pub const HEADPHONE: char = '\u{E7F6}';
    pub const ADD: char = '\u{E710}';
    pub const CLOSE: char = '\u{E711}';
    pub const BOX: char = '\u{E739}';
    pub const BOX_ON: char = '\u{E73A}';
    pub const UNDO: char = '\u{E7A7}';
    pub const RESET: char = '\u{E72C}';
    pub const MINUS: char = '\u{E738}';
    pub const DOC: char = '\u{E7C3}';
    pub const CHECK: char = '\u{E73E}';
    pub const CLIP: char = '\u{E77F}';
    pub const DOWNLOAD: char = '\u{E896}';
    pub const LEFT: char = '\u{E76B}';
    pub const RIGHT: char = '\u{E76C}';
    pub const MIC: char = '\u{E720}';
    pub const GEAR: char = '\u{E713}';
    pub const COPY: char = '\u{E8C8}';
    pub const PASTE: char = '\u{E77F}';
    pub const PALETTE: char = '\u{E790}';
    pub const INFO: char = '\u{E946}';
    pub const POWER: char = '\u{E7E8}';
    pub const KEYBOARD: char = '\u{E765}';
    pub const CAMERA: char = '\u{E722}';
    pub const EDIT: char = '\u{E70F}';
    pub const QR: char = '\u{ED14}';
    pub const LINK: char = '\u{E71B}';
}

// ------------------------------------------------------------------ types

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Media,
    Focus,
    Water,
    Shelf,
    Tasks,
    Clipboard,
    Settings,
}
const TABS: [Tab; 7] = [Tab::Media, Tab::Focus, Tab::Water, Tab::Shelf, Tab::Tasks, Tab::Clipboard, Tab::Settings];

/// The switches on the Settings page.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Opt {
    MiniLyrics,
    LineMode,
    MicMute,
    Shots,
    Autostart,
    /// true: close only on a click outside
    Close(bool),
    /// the Colors page (pick colors without editing a file)
    Theme,
    /// open theme.conf in a text editor (every option)
    ThemeFile,
    Update,
    /// pick a new mute shortcut (the next keys pressed)
    Shortcut,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ViewKey {
    Compact(Live),
    /// line mode: just a thin line at the top edge until the mouse comes
    Line,
    Toast(u64),
    Expanded(Tab, bool),
}

/// Number fields that can be typed into directly.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    FocusMin,
    BreakMin,
    Bottle,
    Goal,
    Interval,
    DrinkMl,
    TaskTime,
    /// a #RRGGBB color on the Colors page
    Hex,
}

/// An in-progress edit: field, typed text, and whether the next key replaces the text.
struct Edit {
    field: Field,
    buf: String,
    fresh: bool,
}

/// What the closed island is showing. Priority: media > focus timer > task due soon > clock.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Live {
    Idle,
    Media,
    /// media with the current line of its synced lyrics (mini lyrics)
    Lyrics,
    Pomo,
    Task,
}

// compact-island metrics (DIP)
const PILL_H: f32 = 32.0;
/// track key of the made-up song in the mini lyrics demo
const DEMO_KEY: u64 = 42;
/// mini lyrics: text size and the limits of the lyric line's width
const MINI_SIZE: f32 = 13.0;
const MINI_MIN_W: f32 = 170.0;
const MINI_MAX_W: f32 = 400.0;
const TASK_SOON_MIN: i64 = 15;
/// Early task reminders: how long before the deadline, and how the notice says it.
/// Bit i of `Task::alerts` marks entry i as shown.
pub const REMINDERS: [i64; 5] = [86400, 6 * 3600, 3 * 3600, 3600, 1800];

/// "in 1 day", "in 6 hours", "in 40 minutes": how far off a deadline `left` seconds away is.
fn left_label(left: i64) -> String {
    let plural = |n: i64, w: &str| if n == 1 { format!("in 1 {w}") } else { format!("in {n} {w}s") };
    if left >= 86400 - 1800 {
        plural(((left + 43200) / 86400).max(1), "day")
    } else if left >= 3600 - 300 {
        plural(((left + 1800) / 3600).max(1), "hour")
    } else {
        plural(((left + 30) / 60).max(1), "minute")
    }
}
/// A task put on a later day without a time is due at 09:00.
const DEFAULT_TIME: i64 = 9 * 3600;

/// The reminders that are already too late for a task due at `due` (a task added 2 hours
/// before its deadline does not get the 1-day, 6-hour and 3-hour notices afterwards).
pub fn passed_alerts(due: Option<i64>, now: i64) -> u8 {
    let Some(d) = due else { return 0 };
    let left = d - now;
    REMINDERS.iter().enumerate().filter(|(_, s)| left <= **s).fold(0, |m, (i, _)| m | (1 << i))
}

/// Which early reminder to show now for a task due at `due`: the index of the closest one that
/// has come up, if it was not shown yet, plus the new `alerts` bits. Overdue tasks get none
/// (the "due now" notice covers them).
pub fn reminder_due(due: i64, now: i64, alerts: u8) -> (Option<usize>, u8) {
    let reached = passed_alerts(Some(due), now);
    if due <= now {
        return (None, alerts | reached);
    }
    if reached & !alerts == 0 {
        return (None, alerts);
    }
    let closest = (0..REMINDERS.len()).rev().find(|i| reached & (1 << i) != 0);
    (closest, alerts | reached)
}
/// Clock in the closed island: bold, sized so the digits sit about as tall as the artwork.
const CLOCK_SIZE: f32 = 18.5;
/// Width of the closed island when nothing is going on: just an empty black notch.
const IDLE_W: f32 = 110.0;
// hover on the closed island: how much it grows, and the intent delays that keep a cursor just
// passing by (on its way to a browser tab) from waking it, and stop flicker at the edge
const HOVER_W: f32 = 16.0;
const HOVER_H: f32 = 4.0;
const HOVER_IN: Duration = Duration::from_millis(50);
const HOVER_OUT: Duration = Duration::from_millis(120);
/// critically damped (no overshoot): settles in ~180 ms growing, ~250 ms shrinking
const HOVER_W_IN: f32 = 26.0;
const HOVER_W_OUT: f32 = 18.0;
// line mode: the thin line the closed island shrinks to, and how long it stays open after
// the mouse leaves
const LINE_W: f32 = 96.0;
const LINE_H: f32 = 5.0;
const LINE_OUT: Duration = Duration::from_millis(200);
/// line <-> pill spring: critically damped, within 1 px of its target after about 250 ms
const LINE_SPRING_W: f32 = 19.0;
/// the pill dropping out of the line: it widens first, then comes down with its content
const LINE_DROP_WIDEN: f32 = 28.0;
const LINE_DROP_FALL: f32 = 16.0;
/// and going back up: it rises first, then narrows into the line
const LINE_RISE: f32 = 24.0;
/// the open panel closes this long after the mouse left it
const PANEL_OUT: Duration = Duration::from_millis(300);
/// how far beside the line (each side) the mouse still opens it: about 1 cm (96 px = 1 inch)
const LINE_HIT_PAD: f32 = 38.0;
// one-line toasts (Caps Lock)
const SHORT_H: f32 = 34.0;
const SHORT_PAD: f32 = 12.0;
const SHORT_ICON: f32 = 18.0;
const SHORT_SIZE: f32 = 13.0;
// two-line notices (Wi-Fi, headphones/TWS, charger, reminders): sized to their text
const TWO_H: f32 = 46.0;
const TWO_PAD: f32 = 9.0;
const TWO_ICON: f32 = 28.0;
const TWO_GAP: f32 = 9.0;
const TWO_TITLE: f32 = 13.5;
const TWO_SUB: f32 = 11.5;
const TWO_MAX_W: f32 = 380.0;
/// Launch / quit / hide transition: the island grows out of (or sinks back into) the top edge.
/// Width the island starts from / shrinks to, as a fraction of its resting width.
const POP_W: f32 = 0.45;
/// Give up waiting for the closing animation after this long.
const LEAVE_MAX: Duration = Duration::from_millis(700);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Leave {
    Quit,
    Hide,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Tab(Tab),
    PlayPause,
    Prev,
    Next,
    Seek(R),
    Lyrics,
    /// lyrics in the closed island on/off
    MiniLyrics,
    LyricLine(f64),
    FocusToggle,
    FocusReset,
    FocusSkip,
    FocusLen(bool, i32),
    WaterAdd(u32),
    WaterUndo,
    WaterSet(u8, i32),
    WaterRemind,
    ShelfItem(usize),
    ShelfRemove(usize),
    ShelfClear,
    ShelfAll,
    ShelfCheck(usize),
    /// the QR badge on a Shelf image: show what its codes say
    QrBadge(usize),
    /// on the QR page: code `i`, open (0) or copy (1); back to the Shelf
    QrDo(usize, u8),
    QrClose,
    /// a button on the QR notice: open (0) or copy (1)
    ToastQr(u8),
    TaskField,
    TaskTime(i64),
    Edit(Field),
    TaskAdd,
    TaskToggle(u64),
    TaskDel(u64),
    /// calendar: previous / next month, back to today, pick a day
    CalMonth(i32),
    CalToday,
    CalDay(i64),
    ClipCopy(usize),
    ClipDel(usize),
    ClipClear,
    ClipCopyAll,
    Toast,
    VolBar(R),
    Opt(Opt),
    /// Colors page: back to Settings, what to color, a ready-made look, the wheel and the
    /// brightness bar (both follow the mouse while held), automatic color, reset everything
    ColBack,
    ColTarget(u8),
    ColPreset(usize),
    ColWheel(R),
    ColBright(R),
    ColAuto,
    ColReset,
    /// copy the color code / use a color code from the clipboard
    ColCopy,
    ColPaste,
    /// one of the quick colors under the code
    ColSwatch(u32),
    /// a category in the Settings sidebar (General, Music, Microphone, Shelf, -, About)
    SetCat(u8),
}

pub enum Deferred {
    DragOut(Vec<String>),
    Menu,
}

enum TIcon {
    G(char),
    /// microphone crossed out
    MicOff,
    Txt(String),
    Battery(f32, bool),
    Drop,
}

const K_INFO: u8 = 0;
const K_VOL: u8 = 1;
const K_BRIGHT: u8 = 2;

/// What tapping a notice does besides opening its tab.
#[derive(Clone, Copy, PartialEq)]
enum Tap {
    None,
    /// download and install the update it announces
    Update,
    /// open the releases page (the automatic update failed)
    Releases,
    /// show the QR codes it announces on the Shelf
    Qr,
}

struct Toast {
    id: u64,
    tap: Tap,
    kind: u8,
    icon: TIcon,
    title: String,
    sub: String,
    level: Option<f32>,
    accent: C,
    until: Instant,
    tab: Option<Tab>,
}

enum Lyr {
    None,
    Loading,
    Missing,
    Found(Vec<Line>),
}

#[derive(Clone, Copy, PartialEq)]
enum PMode {
    Idle,
    Focus,
    Break,
}

struct Pomo {
    mode: PMode,
    end: Instant,
    left: f64,
    total: f64,
    paused: bool,
}

impl Pomo {
    fn active(&self) -> bool {
        self.mode != PMode::Idle
    }
    fn remaining(&self) -> f64 {
        if self.paused {
            self.left
        } else {
            self.end.saturating_duration_since(Instant::now()).as_secs_f64()
        }
    }
}

#[derive(Clone, Copy)]
struct Spring {
    x: f32,
    v: f32,
    t: f32,
}

impl Spring {
    fn new(x: f32) -> Spring {
        Spring { x, v: 0.0, t: x }
    }
    fn step(&mut self, dt: f32) {
        let k: f32 = 300.0;
        self.step_with(dt, k, 2.0 * k.sqrt() * 0.74);
    }
    /// Critically damped at angular speed `w`: quick and without any bounce.
    fn step_fast(&mut self, dt: f32, w: f32) {
        self.step_with(dt, w * w, 2.0 * w);
    }
    fn step_with(&mut self, dt: f32, k: f32, c: f32) {
        let a = -k * (self.x - self.t) - c * self.v;
        self.v += a * dt;
        self.x += self.v * dt;
    }
    fn done(&self) -> bool {
        (self.x - self.t).abs() < 0.2 && self.v.abs() < 3.0
    }
    fn snap(&mut self) {
        self.x = self.t;
        self.v = 0.0;
    }
}

struct Slide {
    from_tab: Tab,
    from_lyr: bool,
    from_w: f32,
    from_h: f32,
    /// +1 slides left (moving to a later tab), -1 slides right
    dir: f32,
    t: f32,
    /// a page inside the same tab (Settings <-> Colors): a short glide with a cross-fade
    /// instead of the full carousel
    sub: bool,
}

const SLIDE_SECS: f32 = 0.34;

/// The artwork travelling between the open island and the closed pill (both ways), while the
/// old view's content fades out and the new one fades in.
#[derive(Clone, Copy)]
struct Morph {
    /// where the artwork was (relative to the screen's center) and its corner radius
    from: R,
    from_rad: f32,
    /// island height when it started: progress is measured by the height
    h0: f32,
    /// the view being left, laid out at its own size while it fades
    old: ViewKey,
    old_w: f32,
    old_h: f32,
}
/// spring speed while morphing: critically damped (no bounce), settles in about 0.4 s
const MORPH_W: f32 = 16.0;
/// how long the old view takes to fade out
const MORPH_FADE: f32 = 0.16;
/// clock <-> lyric line in the closed pill
const MINI_SECS: f32 = 0.55;

struct Ui {
    hits: Vec<(R, Act)>,
    mouse: Option<(f32, f32)>,
}

impl Ui {
    fn hover(&self, r: R) -> bool {
        self.mouse.map(|m| r.contains(m)).unwrap_or(false)
    }
    fn hit(&mut self, r: R, a: Act) -> bool {
        self.hits.push((r, a));
        self.hover(r)
    }
}

/// Sent by a second launch (e.g. from Windows Search) to bring the island back.
pub const WM_APP_SHOW: u32 = WM_APP + 3;
/// `veyra.exe --quit`: the running copy plays its closing animation, saves and exits.
pub const WM_APP_QUIT: u32 = WM_APP + 4;

const TIMER_ANIM: usize = 1;
const TIMER_TICK: usize = 2;

// ------------------------------------------------------------------ app

pub struct App {
    pub hwnd: HWND,
    gfx: Gfx,
    pub data: Data,
    dirty: bool,
    mon_cx: i32,
    mon_top: i32,
    sw: Spring,
    last_min: i64,
    sh: Spring,
    sr: Spring,
    view: ViewKey,
    view_age: f32,
    /// content size for `view`, worked out together with it (see draw)
    view_wh: Option<(f32, f32)>,
    /// the line came from the closed pill (its content fades out as it shrinks); from the open
    /// island or a notice it shrinks empty
    line_from_pill: bool,
    /// the pill came down out of the line: its content slides down with it instead of fading in
    pill_from_line: bool,
    /// the screen's center in window coordinates: fixed on screen, unlike the center of an
    /// island whose width moves by whole pixels
    screen_cx: f32,
    /// right after closing in line mode: do not treat the cursor as hovering (see tick)
    no_hover_until: Option<Instant>,
    expanded: bool,
    tab: Tab,
    lyrics_open: bool,
    hits: Vec<(R, Act)>,
    mouse: Option<(f32, f32)>,
    tracking: bool,
    press: Option<(Option<Act>, f32, f32)>,
    media: Option<MediaState>,
    art: Option<ID2D1Bitmap>,
    art_color: C,
    art_key: u64,
    pending_play: Option<(bool, Instant)>,
    media_tx: Sender<MediaCmd>,
    lyr_key: u64,
    lyr: Lyr,
    lyr_scroll: f32,
    pomo: Pomo,
    toast: Option<Toast>,
    toast_seq: u64,
    audio: Option<system::Audio>,
    caps: bool,
    lang: u16,
    net: Option<bool>,
    net_check: Option<Instant>,
    power: Option<(bool, u8)>,
    low_warned: bool,
    bright_seen: bool,
    input: String,
    input_focus: bool,
    surrogate: Option<u16>,
    task_due: Option<i64>,
    task_scroll: usize,
    /// calendar: the month shown (year, 1-12) and the picked day (day number)
    cal_month: (i64, i64),
    sel_day: i64,
    /// clipboard history, newest first (memory only)
    clips: Vec<clip::Item>,
    clip_scroll: usize,
    /// which history entry was copied back from the list, and when (for the "Copied" mark)
    clip_copied: Option<(Instant, String)>,
    /// text just dropped onto the island (its row says "Saved" for a moment)
    clip_dropped: Option<(Instant, String)>,
    /// when "Copy all" last copied the whole list (its button says "Copied" for a moment)
    clip_all_at: Option<Instant>,
    /// text Veyra itself just put on the clipboard: its echo must not reorder the history
    clip_ignore: Option<String>,
    /// the clipboard changed: read it at this time (see on_clipboard)
    clip_due: Option<Instant>,
    /// when the last "Copied" notice was shown
    copied_at: Option<Instant>,
    /// the context menu of the island is open
    menu_hold: bool,
    /// new screenshots -> Shelf
    shots: shots::Watcher,
    shots_next: Instant,
    shelf_scroll: usize,
    /// animated shelf position (in tiles), slides toward `shelf_scroll`
    shelf_x: f32,
    drag_hover: bool,
    /// the drag over the island carries text (it goes to the Clipboard tab)
    drag_text: bool,
    /// Shelf image previews by path; None while decoding (or when the file is not readable)
    thumbs: HashMap<String, Option<ID2D1Bitmap>>,
    /// what the QR codes in Shelf images say, by path (empty: none); None while looking
    qr: HashMap<String, Option<Vec<String>>>,
    /// the QR notice showing: (its toast id, image path)
    qr_toast: Option<(u64, String)>,
    /// found while a full-screen app hid the island: announced once it is back
    qr_later: Option<String>,
    /// the Shelf image whose QR codes are showing in place of the tiles
    qr_pop: Option<String>,
    /// a QR code just copied on that page (which one, when)
    qr_copied: Option<(Instant, usize)>,
    drag_expanded: bool,
    collapse_at: Option<Instant>,
    /// the mouse has been over the panel since it opened (a panel opened from the tray, with
    /// the cursor far away, must not close by itself right away)
    panel_entered: bool,
    hidden: bool,
    anim_on: bool,
    last_frame: Instant,
    t0: Instant,
    last_sec: Instant,
    tick_ms: u32,
    water_mark: Instant,
    win: (i32, i32, i32, i32),
    isl_px: (i32, i32, i32, i32),
    demo: bool,
    demo_idle: bool,
    slide: Option<Slide>,
    /// Selected shelf paths. Dragging a selected tile carries the whole selection.
    shelf_sel: Vec<String>,
    /// hidden by the user from the menu (separate from auto-hiding in full screen)
    pub user_hidden: bool,
    edit: Option<Edit>,
    demo_switch: Option<Instant>,
    /// testing aid (VEYRA_DEMO=stress): next step and how many were done
    demo_stress: Option<(Instant, u32)>,
    pub deferred: Option<Deferred>,
    pub autostart: bool,
    /// closing animation in progress: what to do at the end, when it started, target width
    leaving: Option<(Leave, Instant, f32)>,
    /// when to hand buffers and idle pages back to Windows after the island got small again
    trim_at: Option<Instant>,
    /// start and frame count of the running animation (debug log)
    anim_stat: (Instant, u32),
    /// the running animation changed what is shown (not just a hover): trim memory afterwards
    anim_view_change: bool,
    /// hover progress 0..1 and its velocity, driven by its own spring
    hov: f32,
    hov_v: f32,
    /// hover as decided after the intent delays
    hover_on: bool,
    /// when the cursor entered / left the island
    hover_in: Option<Instant>,
    hover_out: Option<Instant>,
    /// last seen modification time of theme.json (reloaded when it changes)
    theme_mtime: Option<std::time::SystemTime>,
    /// Settings ▸ Colors: showing, what is being colored (0 background, 1 open island,
    /// 2 highlights), the picked hue / saturation / brightness, the theme.conf text being edited
    colors_open: bool,
    /// the Settings category showing
    settings_cat: u8,
    col_target: u8,
    col_hsv: (f32, f32, f32),
    col_text: String,
    /// the color wheel image, made once per pixel size
    col_wheel: std::cell::RefCell<Option<(u32, ID2D1Bitmap)>>,
    /// what came of the last copy / paste on the Colors page, shown in the code box for a moment
    col_note: Option<(&'static str, bool, Instant)>,
    /// how far the open (expanded) colors are faded in, 0..1
    open_k: f32,
    /// a long line under the mouse is sliding (keeps frames coming), and which one since when
    marquee_on: std::cell::Cell<bool>,
    marquee_at: std::cell::Cell<Option<(u64, Instant)>>,
    /// mini lyrics: width of the lyric text area for the lyrics of this track key
    mini_w: std::cell::Cell<Option<(u64, f32)>>,
    /// last lyric width and line shown, kept while the pill turns back into the clock
    mini_last_w: std::cell::Cell<f32>,
    mini_last_txt: std::cell::RefCell<String>,
    /// clock -> lyric line in the closed pill, 0..1 (linear; eased when used)
    mini_p: f32,
    morph: Option<Morph>,
    /// the artwork drawn in the last frame (relative to the screen's center) and its radius
    art_last: std::cell::Cell<Option<(R, f32)>>,
    /// where the current view puts the artwork while a morph draws it instead
    art_slot: std::cell::Cell<Option<(R, f32)>>,
    /// 0: draw the artwork, 1: only note where it goes (morph), 2: leave it out (old view)
    art_mode: std::cell::Cell<u8>,
    /// the island's shape as drawn this frame, hover growth included
    shape_r: std::cell::Cell<R>,
    /// the microphones: muting them, and whether they are muted
    privacy: privacy::Watcher,
    mic_muted: Option<bool>,
    /// the mute shortcut Veyra holds (None: none could be had)
    hotkey: Option<String>,
    /// Settings: waiting for the keys of a new mute shortcut
    hk_capture: bool,
    /// what happened to the last one picked, shown in its row for a moment (text, good news)
    hk_msg: Option<(String, bool, Instant)>,
    /// testing aid: a scripted step at a given time (see tick)
    demo_step: Option<(Instant, &'static str)>,
    demo_lyrics: Lyr,
    anim_stat_seq: u32,
    /// a newer release found on GitHub
    upd_avail: Option<update::Release>,
    /// checking or installing right now
    upd_busy: bool,
    /// the user asked: report the result even when there is nothing new
    upd_manual: bool,
    /// next automatic check
    upd_next: Instant,
    /// the version whose "Update available" notice was already shown
    upd_shown: Option<String>,
}

fn monitor() -> (i32, i32, f32) {
    unsafe {
        let hm = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hm, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let r = mi.rcMonitor;
        ((r.left + r.right) / 2, r.top, dx as f32 / 96.0)
    }
}

impl App {
    pub fn new(hwnd: HWND) -> windows::core::Result<App> {
        let (cx, top, scale) = monitor();
        let mut gfx = Gfx::new(scale)?;
        theme::ensure_file();
        let th = theme::load();
        gfx.set_theme(&th);
        let first_run = store::first_run();
        let mut data = store::load();
        let today = now().date();
        if data.water.date != today {
            data.water.date = today.clone();
            data.water.log.clear();
        }
        if data.focus.date != today {
            data.focus.date = today;
            data.focus.sessions = 0;
        }
        if first_run && std::env::var_os("VEYRA_DEMO").is_none() {
            // Fresh install: on by default. Saved at once, so what the user changes later stays.
            tray::set_autostart(true);
            data.settings.screenshots = true;
            store::save(&data);
        }
        let privacy = privacy::Watcher::new();
        let mic_muted = privacy.mic_muted();
        let media_tx = crate::media::spawn();
        let audio = system::Audio::new();
        system::watch_network();
        system::watch_brightness(hwnd);
        system::install_kb_hook();
        let n = Instant::now();
        let app = App {
            hwnd,
            gfx,
            data,
            dirty: false,
            mon_cx: cx,
            mon_top: top,
            sw: Spring::new(40.0),
            last_min: 0,
            sh: Spring::new(20.0),
            sr: Spring::new(10.0),
            view: ViewKey::Compact(Live::Idle),
            view_age: 0.0,
            view_wh: None,
            line_from_pill: false,
            pill_from_line: false,
            screen_cx: 0.0,
            no_hover_until: None,
            expanded: false,
            tab: Tab::Media,
            lyrics_open: false,
            hits: Vec::new(),
            mouse: None,
            tracking: false,
            press: None,
            media: None,
            art: None,
            art_color: MEDIA_C,
            art_key: 0,
            pending_play: None,
            media_tx,
            lyr_key: 0,
            lyr: Lyr::None,
            lyr_scroll: 0.0,
            pomo: Pomo { mode: PMode::Idle, end: n, left: 0.0, total: 1.0, paused: false },
            toast: None,
            toast_seq: 0,
            audio,
            caps: system::caps_state(),
            lang: system::fg_lang(),
            net: Some(system::net_status().0),
            net_check: None,
            power: system::power(),
            low_warned: false,
            bright_seen: false,
            input: String::new(),
            input_focus: false,
            surrogate: None,
            task_due: None,
            task_scroll: 0,
            cal_month: (now().y, now().mo),
            sel_day: now().day(),
            clips: Vec::new(),
            clip_scroll: 0,
            clip_copied: None,
            clip_dropped: None,
            clip_all_at: None,
            clip_ignore: None,
            clip_due: None,
            copied_at: None,
            menu_hold: false,
            shots: shots::Watcher::new(),
            shots_next: Instant::now(),
            shelf_scroll: 0,
            shelf_x: 0.0,
            drag_hover: false,
            drag_text: false,
            thumbs: HashMap::new(),
            qr: HashMap::new(),
            qr_toast: None,
            qr_later: None,
            qr_pop: None,
            qr_copied: None,
            drag_expanded: false,
            collapse_at: None,
            panel_entered: false,
            hidden: false,
            anim_on: false,
            last_frame: n,
            t0: n,
            last_sec: n,
            tick_ms: 0,
            water_mark: n,
            win: (0, 0, 0, 0),
            isl_px: (0, 0, 0, 0),
            demo: false,
            demo_idle: false,
            slide: None,
            shelf_sel: Vec::new(),
            user_hidden: false,
            edit: None,
            demo_switch: None,
            demo_stress: None,
            deferred: None,
            autostart: tray::autostart_enabled(),
            leaving: None,
            trim_at: Some(n + Duration::from_secs(5)),
            anim_stat: (n, 0),
            anim_view_change: false,
            hov: 0.0,
            hov_v: 0.0,
            hover_on: false,
            hover_in: None,
            hover_out: None,
            theme_mtime: theme::modified(),
            colors_open: false,
            settings_cat: 0,
            col_target: 0,
            col_hsv: (0.0, 0.0, 0.0),
            col_text: String::new(),
            col_wheel: std::cell::RefCell::new(None),
            col_note: None,
            open_k: 0.0,
            marquee_on: std::cell::Cell::new(false),
            marquee_at: std::cell::Cell::new(None),
            mini_w: std::cell::Cell::new(None),
            mini_last_w: std::cell::Cell::new(MINI_MIN_W),
            mini_last_txt: std::cell::RefCell::new(String::new()),
            mini_p: 0.0,
            morph: None,
            art_last: std::cell::Cell::new(None),
            art_slot: std::cell::Cell::new(None),
            art_mode: std::cell::Cell::new(0),
            shape_r: std::cell::Cell::new(R::default()),
            privacy,
            mic_muted,
            hotkey: None,
            hk_capture: false,
            hk_msg: None,
            demo_step: None,
            demo_lyrics: Lyr::None,
            anim_stat_seq: 0,
            upd_avail: None,
            upd_busy: false,
            upd_manual: false,
            upd_next: n + Duration::from_secs(20),
            upd_shown: None,
        };
        clip::listen(hwnd);
        Ok(app)
    }

    pub fn start(&mut self) {
        self.set_tick(250);
        self.register_mic_hotkey();
        if let Ok(v) = std::env::var("VEYRA_DEMO") {
            self.demo_view(&v);
            // testing aid: VEYRA_MOUSE=x,y (window pixels) pretends the cursor rests there, to see hovers
            if let Some((x, y)) = std::env::var("VEYRA_MOUSE").ok().and_then(|m| {
                let (x, y) = m.split_once(',')?;
                Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
            }) {
                self.mouse = Some((x / self.gfx.scale, y / self.gfx.scale));
            }
        }
        self.pop_in();
        self.kick();
    }

    /// Opening transition: start as a thin sliver at the top edge and spring out to size.
    fn pop_in(&mut self) {
        let (tw, _, _, _) = self.target();
        self.sw = Spring::new(tw * POP_W);
        self.sh = Spring::new(0.0);
        self.sr = Spring::new(0.0);
        self.view_age = 0.0;
        self.kick();
    }

    /// Closing transition: fold the island back into the top edge, then quit or hide.
    fn leave(&mut self, what: Leave) {
        if self.hidden {
            // nothing on screen to animate
            self.finish_leave(what);
            return;
        }
        if self.leaving.is_none() {
            self.collapse();
            self.toast = None;
            let (tw, _, _, _) = self.target();
            self.leaving = Some((what, Instant::now(), tw * POP_W));
        }
        self.kick();
    }

    pub fn begin_quit(&mut self) {
        self.leave(Leave::Quit);
    }

    fn finish_leave(&mut self, what: Leave) {
        dbg_log(&format!("leave: done ({what:?})"));
        self.leaving = None;
        match what {
            // WM_CLOSE goes through DestroyWindow -> WM_DESTROY, which saves the data
            Leave::Quit => unsafe {
                // stay hidden for the few messages left before the window goes away
                self.user_hidden = true;
                self.hidden = true;
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Leave::Hide => {
                self.user_hidden = true;
                self.apply_visibility();
            }
        }
    }

    /// VEYRA_THEME="text = #FF8A3D; icon = auto": theme options for this run only (not saved).
    fn demo_theme(&mut self) {
        if let Ok(s) = std::env::var("VEYRA_THEME") {
            let kv: Vec<(String, String)> = s.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
            let kv: Vec<(&str, String)> = kv.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            self.col_set(&kv, false);
        }
    }

    /// Screenshot/testing aid: VEYRA_DEMO=media|lyrics|focus|water|shelf|tasks|toast|volume|caps|capsoff|stress
    fn demo_view(&mut self, v: &str) {
        self.demo = true;
        self.demo_theme();
        match v {
            "lyrics" => {
                self.lyrics_open = true;
                self.expand(Some(Tab::Media));
                self.ensure_lyrics();
            }
            "media" => self.expand(Some(Tab::Media)),
            // closed island with mini lyrics: a made-up track and lyrics
            "minilyrics" | "minilyricsline" => {
                self.demo_mini();
                if v == "minilyricsline" {
                    self.data.settings.line_mode = true;
                }
            }
            "minilyricsbtn" => {
                self.demo_mini();
                self.expand(Some(Tab::Media));
            }
            // transitions (watch with VEYRA_DUMP_SEQ): the lyrics arrive while closed / the open
            // island closes into the lyric pill / the lyric pill opens
            "minimorph" => {
                self.demo_mini();
                self.demo_lyrics = std::mem::replace(&mut self.lyr, Lyr::Loading);
                self.demo_step = Some((Instant::now() + Duration::from_millis(700), "found"));
            }
            "minimorphline" => {
                self.demo_mini();
                self.data.settings.line_mode = true;
                self.demo_lyrics = std::mem::replace(&mut self.lyr, Lyr::Loading);
                self.demo_step = Some((Instant::now() + Duration::from_millis(700), "found"));
            }
            "minicollapse" => {
                self.demo_mini();
                self.expand(Some(Tab::Media));
                self.demo_step = Some((Instant::now() + Duration::from_millis(900), "collapse"));
            }
            // the mouse comes onto the pill (and leaves again) while it shows lyrics
            "settings" => self.expand(Some(Tab::Settings)),
            // Settings, then the Colors page glides in (for VEYRA_DUMP_SEQ)
            "settingstocolors" => {
                self.expand(Some(Tab::Settings));
                self.demo_step = Some((Instant::now() + Duration::from_millis(900), "tocolors"));
            }
            "settingsabout" => {
                self.settings_cat = 5;
                self.expand(Some(Tab::Settings));
            }
            // the Colors page; "colorslight" shows a light look without saving it
            // a ready-made look on the Music page, not saved: VEYRA_DEMO=look3 (Plum), look5 (Night), ...
            l if l.starts_with("look") => {
                let i: usize = l[4..].parse().unwrap_or(0);
                if let Some(look) = PRESETS.get(i) {
                    self.col_set(&preset_keys(look), false);
                }
                // VEYRA_THEME then goes on top of the look
                self.demo_theme();
                self.expand(Some(Tab::Media));
            }
            "colors" | "colorsaccent" | "colorslight" | "colorswater" | "colorstext" | "colorsback" => {
                self.expand(Some(Tab::Settings));
                self.open_colors();
                if v == "colorstext" {
                    self.col_target = COL_TEXT;
                    self.col_set(&[("text", "#FFD6A5".into()), ("expanded_text", "same".into())], false);
                    self.col_hsv = theme::to_hsv(self.col_color());
                }
                if v == "colorsback" {
                    // look at another feature, then come back to Settings: still on Colors
                    self.act(Act::Tab(Tab::Water), 0.0);
                    self.act(Act::Tab(Tab::Settings), 0.0);
                }
                if v == "colorswater" {
                    self.col_target = 5;
                    self.col_set(&[("color_water", "#3DDC97".into())], false);
                    self.col_hsv = theme::to_hsv(self.col_color());
                }
                if v == "colorsaccent" {
                    self.col_target = 2;
                    self.col_hsv = theme::to_hsv(self.col_color());
                }
                if v == "colorslight" {
                    self.col_set(&preset_keys(&PRESETS[7]), false);
                    self.col_hsv = theme::to_hsv(self.col_color());
                }
            }
            "settingskeys" => {
                self.expand(Some(Tab::Settings));
                self.settings_cat = 2;
                self.hk_capture = true;
            }
            "settingstaken" => {
                self.expand(Some(Tab::Settings));
                self.settings_cat = 2;
                self.hk_capture = true;
                self.hk_msg = Some(("Ctrl+Alt+M is taken by another app".into(), false, Instant::now() + Duration::from_secs(60)));
            }
            "micoff" => self.toast(TIcon::MicOff, "Microphone off", "", 600.0, RED, None),
            "minihover" => {
                self.demo_mini();
                self.demo_step = Some((Instant::now() + Duration::from_millis(900), "hover"));
            }
            "miniexpand" => {
                self.demo_mini();
                self.demo_step = Some((Instant::now() + Duration::from_millis(900), "expand"));
            }
            "focus" => {
                self.start_phase(PMode::Focus);
                self.expand(Some(Tab::Focus));
            }
            "water" => {
                if self.data.water.log.is_empty() {
                    self.data.water.log = vec![500, 250];
                }
                self.expand(Some(Tab::Water));
            }
            "shelf" | "shelfall" => {
                if v == "shelfall" {
                    self.shelf_sel = self.data.shelf.iter().take(2).cloned().collect();
                }
                if self.data.shelf.is_empty() {
                    let win = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
                    self.data.shelf = vec![format!(r"{win}\notepad.exe"), format!(r"{win}\win.ini"), format!(r"{win}\System32")];
                }
                self.expand(Some(Tab::Shelf));
            }
            // image previews: a few files from VEYRA_DEMO_DIR next to a non-image file
            "shelfimg" => {
                if let Some(dir) = std::env::var_os("VEYRA_DEMO_DIR") {
                    let mut imgs: Vec<String> = std::fs::read_dir(dir)
                        .map(|r| r.flatten().map(|e| e.path().to_string_lossy().into_owned()).filter(|p| thumbs::is_image(p)).take(4).collect())
                        .unwrap_or_default();
                    let win = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
                    imgs.push(format!(r"{win}\win.ini"));
                    self.data.shelf = imgs;
                }
                self.expand(Some(Tab::Shelf));
            }
            // QR codes in pictures from VEYRA_DEMO_DIR: the notice (qr), the badges on the
            // Shelf (qrshelf), the codes of the first one (qrpage)
            "qr" | "qrshelf" | "qrpage" => {
                if let Some(dir) = std::env::var_os("VEYRA_DEMO_DIR") {
                    let mut imgs: Vec<String> = std::fs::read_dir(dir)
                        .map(|r| r.flatten().map(|e| e.path().to_string_lossy().into_owned()).filter(|p| thumbs::is_image(p)).take(4).collect())
                        .unwrap_or_default();
                    imgs.sort();
                    for p in &imgs {
                        self.qr.insert(p.clone(), Some(qr::scan_now(p)));
                    }
                    self.data.shelf = imgs;
                }
                let first = self.data.shelf.iter().find(|p| self.qr.get(*p).and_then(|c| c.as_ref()).map(|c| !c.is_empty()).unwrap_or(false)).cloned();
                match (v, first) {
                    ("qr", Some(p)) => self.qr_notice(&p),
                    ("qrpage", Some(p)) => self.show_qr_page(&p),
                    _ => self.expand(Some(Tab::Shelf)),
                }
            }
            // text dragged over the island (empty / filled Clipboard tab)
            "cliphover" | "cliphoverfull" => {
                if v == "cliphoverfull" {
                    let n = now().secs;
                    clip::push(&mut self.clips, "Meeting moved to 3 pm".into(), n - 300);
                    clip::push(&mut self.clips, "https://example.com/report".into(), n - 60);
                    self.clip_dropped = Some((Instant::now() + Duration::from_secs(60), "https://example.com/report".into()));
                }
                self.drag_hover = true;
                self.drag_text = true;
                self.expand(Some(Tab::Clipboard));
            }
            // leak check: keeps re-fetching lyrics and opening / closing the island
            "stress" => {
                self.lyrics_open = true;
                self.expand(Some(Tab::Media));
                self.ensure_lyrics();
                self.demo_stress = Some((Instant::now() + Duration::from_millis(1500), 0));
            }
            "slide" => {
                self.expand(Some(Tab::Media));
                self.demo_switch = Some(Instant::now() + Duration::from_millis(1500));
            }
            "tasks" | "tasksday" | "tasksfeb" => {
                // always the samples (demo mode never saves, the real list stays as it is)
                {
                    self.data.tasks.clear();
                    let n = now().secs;
                    let day = n.div_euclid(86400) * 86400;
                    self.data.tasks.push(Task { id: 1, text: "Send the weekly report to the whole team before Friday".into(), due: Some(n + 3600), ..Default::default() });
                    self.data.tasks.push(Task { id: 2, text: "Call the dentist".into(), due: Some(n - 600), ..Default::default() });
                    self.data.tasks.push(Task { id: 3, text: "Buy oat milk".into(), done: true, ..Default::default() });
                    self.data.tasks.push(Task { id: 4, text: "Team meeting".into(), due: Some(day + 86400 + 10 * 3600), ..Default::default() });
                    self.data.tasks.push(Task { id: 5, text: "Submit the assignment".into(), due: Some(day + 5 * 86400 + 23 * 3600 + 59 * 60), ..Default::default() });
                    self.data.tasks.push(Task { id: 6, text: "Pay rent".into(), due: Some(day - 3 * 86400 + 12 * 3600), done: true, ..Default::default() });
                }
                if v == "tasksday" {
                    self.select_day(now().day() + 5);
                }
                // February 2027 starts on a Monday: only 4 weeks of its own
                if v == "tasksfeb" {
                    self.select_day(days_from_civil(2027, 2, 10));
                }
                self.input = "Review pull request".into();
                self.input_focus = true;
                self.adjust_due(0);
                self.expand(Some(Tab::Tasks));
            }
            "clipboard" => {
                let n = now().secs;
                let samples = ["Veyra", "Meeting moved to 3 pm,\nsame room", "https://lrclib.net/docs/api/get-lyrics-by-track-signature-and-duration", "git commit -m \"fix: seam on light themes\""];
                for (i, t) in samples.iter().enumerate() {
                    clip::push(&mut self.clips, t.to_string(), n - 420 * (3 - i as i64));
                }
                self.expand(Some(Tab::Clipboard));
            }
            "line" => {
                self.data.settings.line_mode = true;
            }
            "clipboardempty" => self.expand(Some(Tab::Clipboard)),
            "tasksempty" => {
                self.data.tasks.clear();
                self.expand(Some(Tab::Tasks));
            }
            "toast" => self.toast(TIcon::Drop, "Time to drink some water", "750 / 2000 ml today · tap to log", 600.0, feat(&self.gfx, Tab::Water), Some(Tab::Water)),
            "volume" => {
                self.hud(K_VOL, ic::VOL, 0.64, WHITE);
                if let Some(t) = &mut self.toast {
                    t.until = Instant::now() + Duration::from_secs(600);
                }
            }
            "focuspill" => self.start_phase(PMode::Focus),
            "caps" => self.toast(TIcon::Txt("⇪".into()), "Caps Lock on", "", 600.0, GREEN, None),
            "capsoff" => self.toast(TIcon::Txt("⇪".into()), "Caps Lock off", "", 600.0, WHITE, None),
            "tws" => self.toast(TIcon::G(ic::HEADPHONE), "Headphones connected", "Galaxy Buds2 Pro", 600.0, WHITE, None),
            "copied" => self.toast(TIcon::G(ic::CLIP), "Text copied", "", 600.0, WHITE, None),
            "charging" => self.toast(TIcon::Battery(0.64, true), "Charging", "64% · plugged in", 600.0, GREEN, None),
            "idle" => self.demo_idle = true,
            "editwater" => {
                self.expand(Some(Tab::Water));
                self.edit = Some(Edit { field: Field::Goal, buf: "2.5".into(), fresh: false });
            }
            "editdrink" => {
                self.expand(Some(Tab::Water));
                self.edit = Some(Edit { field: Field::DrinkMl, buf: "330".into(), fresh: false });
            }
            "edittask" => {
                self.input = "Submit assignment".into();
                self.expand(Some(Tab::Tasks));
                self.edit = Some(Edit { field: Field::TaskTime, buf: "1h30".into(), fresh: false });
            }
            // updater test: with VEYRA_UPDATE_FROM=<older version>, checks and installs right away
            "update" => {
                self.upd_busy = true;
                update::check(true);
            }
            "taskremind" => self.toast(TIcon::G(ic::TASKS), "Task due in 1 hour", "Send the weekly report · 14:30", 600.0, feat(&self.gfx, Tab::Tasks), Some(Tab::Tasks)),
            "taskpill" | "taskfocus" => {
                self.demo_idle = true;
                let n = now().secs;
                self.data.tasks = vec![Task { id: 1, text: "Call the dentist".into(), due: Some(n + 8 * 60 + 30), ..Default::default() }];
                if v == "taskfocus" {
                    self.start_phase(PMode::Focus);
                }
            }
            _ => {}
        }
    }

    fn demo_mini(&mut self) {
        let key = DEMO_KEY;
        self.media = Some(MediaState {
            title: "Midnight Drive".into(),
            artist: "Veyra".into(),
            album: String::new(),
            app: "Spotify".into(),
            playing: true,
            pos: 14.0,
            dur: 200.0,
            at: Instant::now(),
            can_seek: true,
            key,
        });
        self.data.settings.mini_lyrics = true;
        // the user's own line mode would change what these demos show
        self.data.settings.line_mode = false;
        self.lyr_key = key;
        let lines = ["City lights are fading in the mirror", "Hold the wheel and never look behind", "We were young and running out of summer", ""];
        self.lyr = Lyr::Found(lines.iter().enumerate().map(|(i, t)| Line { t: 6.0 + i as f64 * 6.0, text: t.to_string() }).collect());
        self.lyr_scroll = 1.0;
    }

    pub fn save_now(&mut self) {
        if self.demo {
            return;
        }
        store::save(&self.data);
        self.dirty = false;
    }

    fn set_tick(&mut self, ms: u32) {
        if self.tick_ms != ms {
            self.tick_ms = ms;
            unsafe { SetTimer(self.hwnd, TIMER_TICK, ms, None) };
        }
    }

    fn kick(&mut self) {
        if !self.anim_on {
            self.anim_on = true;
            self.last_frame = Instant::now();
            self.anim_stat = (self.last_frame, 0);
            if crate::vsync::available() {
                // paced to the display's refresh rate
                crate::vsync::run(true);
            } else {
                unsafe { SetTimer(self.hwnd, TIMER_ANIM, 15, None) };
            }
        }
    }

    fn send(&self, c: MediaCmd) {
        let _ = self.media_tx.send(c);
    }

    fn media_pos(&self) -> f64 {
        match &self.media {
            Some(m) => {
                let mut p = m.pos;
                if m.playing {
                    p += m.at.elapsed().as_secs_f64();
                }
                if m.dur > 0.0 {
                    p.min(m.dur)
                } else {
                    p
                }
            }
            None => 0.0,
        }
    }

    fn cur_line(&self) -> Option<usize> {
        let Lyr::Found(v) = &self.lyr else { return None };
        let p = self.media_pos() + 0.25;
        let i = v.partition_point(|l| l.t <= p);
        if i == 0 {
            None
        } else {
            Some(i - 1)
        }
    }

    fn ensure_lyrics(&mut self) {
        if let Some(m) = &self.media {
            if self.lyr_key != m.key {
                self.lyr_key = m.key;
                self.lyr = Lyr::Loading;
                self.lyr_scroll = 0.0;
                lyrics::fetch(m.key, m.title.clone(), m.artist.clone(), m.album.clone(), m.dur);
            }
        }
    }

    fn cur_art(&self) -> Option<&ID2D1Bitmap> {
        let key = self.media.as_ref()?.key;
        if key == self.art_key {
            self.art.as_ref()
        } else {
            None
        }
    }

    fn accent(&self) -> C {
        if self.cur_art().is_some() {
            self.art_color
        } else {
            MEDIA_C
        }
    }

    /// The timer's color: theme.conf `timer` / `timer_break`, else orange / green.
    fn pomo_color(&self) -> C {
        if self.pomo.mode == PMode::Break {
            self.break_color()
        } else {
            self.focus_color()
        }
    }

    fn focus_color(&self) -> C {
        self.gfx.theme.timer.unwrap_or(FOCUS_C)
    }

    fn break_color(&self) -> C {
        self.gfx.theme.timer_break.unwrap_or(BREAK_C)
    }

    // -------------------------------------------------------------- toasts

    fn toast(&mut self, icon: TIcon, title: &str, sub: &str, secs: f32, accent: C, tab: Option<Tab>) {
        self.toast_seq += 1;
        self.toast = Some(Toast {
            id: self.toast_seq,
            tap: Tap::None,
            kind: K_INFO,
            icon,
            title: title.into(),
            sub: sub.into(),
            level: None,
            accent,
            until: Instant::now() + Duration::from_secs_f32(secs),
            tab,
        });
        self.kick();
    }

    fn hud(&mut self, kind: u8, ch: char, level: f32, accent: C) {
        if self.expanded {
            return;
        }
        let until = Instant::now() + Duration::from_millis(1600);
        if let Some(t) = &mut self.toast {
            if t.kind == kind {
                t.level = Some(level);
                t.icon = TIcon::G(ch);
                t.until = until;
                self.kick();
                return;
            }
        }
        self.toast_seq += 1;
        self.toast = Some(Toast {
            id: self.toast_seq,
            tap: Tap::None,
            kind,
            icon: TIcon::G(ch),
            title: String::new(),
            sub: String::new(),
            level: Some(level),
            accent,
            until,
            tab: None,
        });
        self.kick();
    }

    // -------------------------------------------------------------- expand

    fn expand(&mut self, tab: Option<Tab>) {
        let t = tab.unwrap_or(if self.media.is_some() {
            Tab::Media
        } else if self.pomo.active() {
            Tab::Focus
        } else if self.tab == Tab::Media {
            Tab::Media
        } else {
            self.tab
        });
        self.tab = t;
        if !self.expanded {
            self.panel_entered = self.cursor_inside();
        }
        self.expanded = true;
        self.toast = None;
        self.collapse_at = None;
        if self.data.settings.close_on_click {
            // a click anywhere else closes it, also when Windows did not let it become active
            system::watch_clicks(true);
        }
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
        }
        if self.tab == Tab::Media && self.lyrics_open {
            self.ensure_lyrics();
        }
        self.kick();
    }

    fn collapse(&mut self) {
        self.cancel_hk_capture();
        self.commit_edit();
        self.shelf_sel.clear();
        self.qr_pop = None;
        self.expanded = false;
        self.input_focus = false;
        self.drag_expanded = false;
        system::watch_clicks(false);
        self.collapse_at = None;
        self.press = None;
        if self.data.settings.line_mode {
            // The cursor was over the open panel (Esc) or just clicked outside it: that is not a
            // hover on the line, so close straight to the line. A cursor really resting on the
            // line still opens it with its next movement.
            self.mouse = None;
            self.tracking = false;
            self.hover_on = false;
            self.no_hover_until = Some(Instant::now() + Duration::from_millis(350));
        }
        self.kick();
    }

    // -------------------------------------------------------------- layout

    fn target(&self) -> (f32, f32, f32, ViewKey) {
        if let Some((_, _, w)) = self.leaving {
            // keep the current view so its content simply fades while the shape sinks away
            return (w, 0.0, 0.0, self.view);
        }
        if self.expanded {
            let lyr = self.tab == Tab::Media && self.lyrics_open && self.media.is_some();
            let (w, h) = match self.tab {
                Tab::Media if lyr => (760.0, 214.0),
                Tab::Media => (500.0, 190.0),
                Tab::Focus => (500.0, 190.0),
                Tab::Water => (500.0, 214.0),
                Tab::Shelf => (500.0, 196.0),
                Tab::Tasks => (500.0, 250.0),
                Tab::Clipboard => (500.0, 250.0),
                Tab::Settings if self.colors_open => (500.0, 402.0),
                Tab::Settings => (500.0, 262.0),
            };
            // Settings: the flag says whether the Colors page is showing (fades like lyrics)
            let flag = if self.tab == Tab::Settings { self.colors_open } else { lyr };
            return (w, h, 30.0, ViewKey::Expanded(self.tab, flag));
        }
        if !self.hidden {
            if let Some(t) = &self.toast {
                if t.level.is_some() {
                    return (320.0, 40.0, 16.0, ViewKey::Toast(t.id));
                }
                if t.sub.is_empty() {
                    // one-liners (Caps Lock): a small pill hugging the text
                    let w = SHORT_PAD + SHORT_ICON + 8.0 + self.gfx.measure(&t.title, SHORT_SIZE, 600) + SHORT_PAD + 4.0;
                    return (w, SHORT_H, SHORT_H / 2.0, ViewKey::Toast(t.id));
                }
                let text = self.gfx.measure(&t.title, TWO_TITLE, 600).max(self.gfx.measure(&t.sub, TWO_SUB, 400));
                if let Some(code) = self.qr_toast_code(t.id) {
                    // the QR notice: its buttons on the right, a long link cut short
                    let btns = qr_btns_w(&self.gfx, &code);
                    let w = (TWO_PAD + TWO_ICON + TWO_GAP + text.min(QR_TEXT_MAX) + 12.0 + btns + TWO_PAD).clamp(150.0, TWO_MAX_W + btns);
                    return (w, TWO_H, 20.0, ViewKey::Toast(t.id));
                }
                let w = (TWO_PAD + TWO_ICON + TWO_GAP + text + TWO_PAD + 8.0).clamp(150.0, TWO_MAX_W);
                return (w, TWO_H, 20.0, ViewKey::Toast(t.id));
            }
        }
        if self.data.settings.line_mode && !self.hover_on && !self.drag_hover && !self.hidden && !self.mini_on() {
            return (LINE_W, LINE_H, LINE_H / 2.0, ViewKey::Line);
        }
        let (live, sec) = self.lives();
        // hover growth is added on top in render(), driven by its own spring
        let w = self.compact_width(live) + self.segment_width(sec);
        // clock and lyric line are one view: the pill morphs between them (see mini_p)
        let key = if live == Live::Lyrics { Live::Media } else { live };
        (w, PILL_H, 13.0, ViewKey::Compact(key))
    }

    /// (primary activity, optional secondary activity shown at the right end of the island)
    fn lives(&self) -> (Live, Option<Live>) {
        let mut v = Vec::with_capacity(3);
        if self.media.is_some() {
            v.push(if self.mini_on() { Live::Lyrics } else { Live::Media });
        }
        if self.pomo.active() {
            v.push(Live::Pomo);
        }
        if self.task_soon().is_some() {
            v.push(Live::Task);
        }
        (v.first().copied().unwrap_or(Live::Idle), v.get(1).copied())
    }

    // -------------------------------------------------------------- microphones

    /// Once a second: whether the microphones got muted or unmuted somewhere else (Windows quick
    /// settings, the keyboard's mic key), for the notice and the Settings switch.
    fn poll_privacy(&mut self) {
        if self.demo {
            return;
        }
        let m = self.privacy.mic_muted();
        if m != self.mic_muted {
            let was = self.mic_muted;
            self.mic_muted = m;
            if let (Some(_), Some(now)) = (was, m) {
                self.mic_notice(now);
            }
            self.kick();
        }
    }

    fn mic_notice(&mut self, muted: bool) {
        if muted {
            self.toast(TIcon::MicOff, "Microphone off", "", 1.6, RED, None);
        } else {
            self.toast(TIcon::G(ic::MIC), "Microphone on", "", 1.6, GREEN, None);
        }
    }

    /// Mute or unmute every microphone (the dot, Ctrl+Alt+M, the Settings page).
    fn toggle_mic(&mut self) {
        let Some(muted) = self.privacy.mic_muted() else {
            self.toast(TIcon::G(ic::MIC), "No microphone found", "", 1.8, WHITE, None);
            return;
        };
        let ok = self.privacy.set_mic_muted(!muted);
        dbg_log(&format!("mic: mute {} -> {} ({})", muted, !muted, if ok { "done" } else { "failed" }));
        if ok {
            self.mic_muted = Some(!muted);
            self.mic_notice(!muted);
        }
        self.kick();
    }

    /// Take the mute shortcut: the user's own if it is free, else the first free default.
    fn register_mic_hotkey(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(self.hwnd, HOTKEY_MIC);
        }
        // (demo copies must not take it from the real one)
        if std::env::var_os("VEYRA_DEMO").is_some() {
            let (m, vk) = self.data.settings.mic_hotkey.map(|[m, vk]| (m, vk)).unwrap_or(MIC_SHORTCUTS[0]);
            self.hotkey = Some(hotkey_name(m, vk));
            return;
        }
        let mut own = self.data.settings.mic_hotkey.map(|[m, vk]| (m, vk));
        if let Some((m, vk)) = own {
            if hotkey_allowed(m, vk).is_err() {
                // picked before this check existed (Ctrl+C, say): it would take that key from
                // every app, so back to the default
                dbg_log(&format!("hotkey: dropping {}", hotkey_name(m, vk)));
                self.data.settings.mic_hotkey = None;
                self.dirty = true;
                own = None;
            }
        }
        self.hotkey = own.into_iter().chain(MIC_SHORTCUTS).find_map(|(m, vk)| self.try_hotkey(m, vk));
    }

    fn try_hotkey(&self, mods: u32, vk: u32) -> Option<String> {
        let name = hotkey_name(mods, vk);
        match unsafe { RegisterHotKey(self.hwnd, HOTKEY_MIC, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk) } {
            Ok(()) => {
                dbg_log(&format!("hotkey: {name} registered"));
                Some(name)
            }
            Err(e) => {
                dbg_log(&format!("hotkey: {name} not registered: {e}"));
                None
            }
        }
    }

    /// Settings: the next keys pressed become the mute shortcut. The current one is let go
    /// meanwhile, so pressing it again can pick it (instead of muting).
    fn start_hk_capture(&mut self) {
        self.hk_capture = true;
        self.hk_msg = None;
        unsafe {
            let _ = UnregisterHotKey(self.hwnd, HOTKEY_MIC);
        }
        self.kick();
    }

    fn cancel_hk_capture(&mut self) {
        if self.hk_capture {
            self.hk_capture = false;
            self.register_mic_hotkey();
            self.kick();
        }
    }

    fn hk_message(&mut self, text: String, good: bool) {
        self.hk_msg = Some((text, good, Instant::now()));
    }

    /// A key while picking a shortcut: Esc cancels, Backspace / Delete go back to the default,
    /// a key with Ctrl, Alt or Win held becomes the shortcut if no other app owns it.
    fn hk_key(&mut self, vk: u32) {
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
        if vk == VK_ESCAPE.0 as u32 {
            self.cancel_hk_capture();
            return;
        }
        // the modifiers on their own: wait for the key that goes with them
        if [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN, VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_CAPITAL]
            .iter()
            .any(|k| k.0 as u32 == vk)
        {
            return;
        }
        let mut mods = 0;
        if down(VK_CONTROL) {
            mods |= MOD_CONTROL.0;
        }
        if down(VK_MENU) {
            mods |= MOD_ALT.0;
        }
        if down(VK_SHIFT) {
            mods |= MOD_SHIFT.0;
        }
        if down(VK_LWIN) || down(VK_RWIN) {
            mods |= MOD_WIN.0;
        }
        if mods == 0 && (vk == VK_BACK.0 as u32 || vk == VK_DELETE.0 as u32) {
            self.data.settings.mic_hotkey = None;
            self.dirty = true;
            self.hk_capture = false;
            self.register_mic_hotkey();
            let name = self.hotkey.clone().unwrap_or_default();
            self.hk_message(format!("Back to {name}"), true);
            self.kick();
            return;
        }
        let name = hotkey_name(mods, vk);
        if let Err(why) = hotkey_allowed(mods, vk) {
            // still picking: try another
            self.hk_message(why.replace("{}", &name), false);
            self.kick();
            return;
        }
        if self.demo {
            self.data.settings.mic_hotkey = Some([mods, vk]);
            self.hotkey = Some(name.clone());
            self.hk_capture = false;
            self.hk_message(format!("{name} is set"), true);
        } else if let Some(n) = self.try_hotkey(mods, vk) {
            self.data.settings.mic_hotkey = Some([mods, vk]);
            self.dirty = true;
            self.hotkey = Some(n);
            self.hk_capture = false;
            self.hk_message(format!("{name} is set"), true);
        } else {
            // still picking: try another
            self.hk_message(format!("{name} is taken by another app"), false);
        }
        self.kick();
    }

    pub fn on_hotkey(&mut self, id: i32) {
        dbg_log(&format!("hotkey: {id}"));
        if id == HOTKEY_MIC {
            self.toggle_mic();
        }
    }

    // -------------------------------------------------------------- settings

    fn set_opt(&mut self, o: Opt) {
        match o {
            Opt::MiniLyrics => self.toggle_mini_lyrics(),
            Opt::LineMode => self.tray_cmd(tray::CMD_LINE),
            Opt::MicMute => self.toggle_mic(),
            Opt::Shots => self.tray_cmd(tray::CMD_SHOTS),
            Opt::Autostart => self.tray_cmd(tray::CMD_AUTOSTART),
            Opt::Close(click) => self.tray_cmd(if click { tray::CMD_CLOSE_CLICK } else { tray::CMD_CLOSE_AUTO }),
            Opt::Theme => self.open_colors(),
            Opt::ThemeFile => self.tray_cmd(tray::CMD_THEME),
            Opt::Update => self.tray_cmd(tray::CMD_UPDATE),
            Opt::Shortcut => {
                if self.hk_capture {
                    self.cancel_hk_capture();
                } else {
                    self.start_hk_capture();
                }
            }
        }
        self.kick();
    }

    /// Settings, laid out like Windows Settings: categories down the left, the chosen one's
    /// options on the right as cards (icon, name, a short explanation, the switch or button).
    fn draw_settings(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let s = &self.data.settings;
        // sidebar
        let sw = 128.0;
        let cats: [(char, &str); 6] = [
            (ic::GEAR, "General"),
            (ic::MUSIC, "Music"),
            (ic::MIC, "Microphone"),
            (ic::FOLDER, "Shelf"),
            (ic::PALETTE, "Colors"),
            (ic::INFO, "About"),
        ];
        let ih = 29.0;
        for (i, (glyph, name)) in cats.iter().enumerate() {
            let b = R::new(c.x - 4.0, c.y - 6.0 + i as f32 * (ih + 3.0), sw, ih);
            let on = self.settings_cat as usize == i;
            let act = if i == 4 { Act::Opt(Opt::Theme) } else { Act::SetCat(i as u8) };
            let hv = ui.hit(b, act);
            if on {
                g.rrect(b, 8.0, surf_active(g));
                // the little bar Windows puts beside the chosen category
                g.rrect(R::new(b.x + 1.0, b.cy() - 7.0, 3.0, 14.0), 1.5, g.theme.feature[6].unwrap_or(WATER_C));
            } else if hv {
                g.rrect(b, 8.0, surf(g, false));
            }
            let col = if on || hv { WHITE } else { T2 };
            g.icon(*glyph, R::new(b.x + 10.0, b.y, 18.0, b.h), 13.0, btn_icon(g, col));
            g.text(name, R::new(b.x + 36.0, b.y, b.w - 40.0, b.h), 12.0, if on { 600 } else { 500 }, btn_fg(g, col), Al::L);
        }

        // the chosen category: its name, then its cards
        let x = c.x + sw + 10.0;
        let w = c.right() - x;
        let cat = (self.settings_cat as usize).min(5);
        g.text(cats[cat].1, R::new(x + 2.0, c.y - 8.0, w, 24.0), 16.0, 700, WHITE, Al::L);
        let ch = 46.0;
        let card = |i: usize| R::new(x, c.y + 24.0 + i as f32 * (ch + 4.0), w, ch);
        match cat {
            0 => {
                setting_card(g, ui, card(0), ic::POWER, "Start with Windows", "Opens when you sign in", Some(self.autostart), Opt::Autostart, GREEN);
                setting_card(g, ui, card(1), ic::MINUS, "Shrink to a line", "A thin line until the mouse comes", Some(s.line_mode), Opt::LineMode, GREEN);
                // how the open island closes: two choices on the right
                let r = card(2);
                card_base(g, r, false);
                card_text(g, r, ic::CLOSE, "Closes when", "The open island closes", 150.0);
                let bw = 74.0;
                for (k, (label, click)) in [("Mouse leaves", false), ("Click outside", true)].into_iter().enumerate() {
                    let b = R::new(r.right() - 10.0 - (2 - k) as f32 * (bw + 4.0) + 4.0, r.cy() - 13.0, bw, 26.0);
                    let on = s.close_on_click == click;
                    let hv = ui.hit(b, Act::Opt(Opt::Close(click)));
                    g.rrect(b, 13.0, if on { surf_active(g) } else { surf(g, hv) });
                    g.text(label, b, 11.0, if on { 600 } else { 500 }, if on { WHITE } else { T2 }, Al::C);
                }
            }
            1 => {
                setting_card(g, ui, card(0), ic::LYRICS, "Lyrics in the closed island", "The current line shows in the pill", Some(s.mini_lyrics), Opt::MiniLyrics, GREEN);
            }
            2 => {
                setting_card(g, ui, card(0), ic::MIC, "Mute all microphones", "Every microphone at once", self.mic_muted, Opt::MicMute, RED);
                // the shortcut: click the button, then press the new keys
                let r = card(1);
                card_base(g, r, false);
                let cap = self.hk_capture;
                let val = if cap { "Press keys\u{2026}".to_string() } else { self.hotkey.clone().unwrap_or_else(|| "None free".into()) };
                let bw = g.measure(&val, 12.0, 600) + 24.0;
                let b = R::new(r.right() - 10.0 - bw, r.cy() - 13.0, bw, 26.0);
                card_text(g, r, ic::KEYBOARD, "Mute shortcut", "Works from any app", b.x - r.x - 10.0);
                let hv = ui.hit(b, Act::Opt(Opt::Shortcut));
                if cap {
                    pill_btn(g, ui, b, &val, accent_fg(g), accent_btn(g, feat(g, Tab::Settings)), Act::Opt(Opt::Shortcut));
                } else {
                    pill_btn(g, ui, b, &val, btn_fg(g, WHITE), surf(g, hv), Act::Opt(Opt::Shortcut));
                }
                // under the cards: how to pick, or what came of the last pick
                let hr = R::new(x + 4.0, card(2).y, w - 8.0, 18.0);
                let msg = self.hk_msg.as_ref().filter(|(_, _, at)| at.elapsed() < Duration::from_secs(3));
                if let Some((t, good, _)) = msg {
                    g.text(t, hr, 11.5, 600, if *good { GREEN } else { RED }, Al::L);
                } else if cap {
                    g.text("Two of Ctrl, Alt, Shift, Win + a key \u{00B7} Esc cancels \u{00B7} \u{232B} default", hr, 11.0, 500, T2, Al::L);
                }
            }
            3 => {
                setting_card(g, ui, card(0), ic::CAMERA, "Add screenshots to Shelf", "New screenshots land on the Shelf", Some(s.screenshots), Opt::Shots, GREEN);
            }
            _ => {
                // version and updates
                let r = card(0);
                card_base(g, r, false);
                let upd = match (&self.upd_avail, self.upd_busy) {
                    (_, true) => "Updating\u{2026}".to_string(),
                    (Some(r), false) => format!("Update to v{}", r.version),
                    (None, false) => "Check for updates".to_string(),
                };
                let uw = g.measure(&upd, 12.0, 600) + 26.0;
                let ub = R::new(r.right() - 10.0 - uw, r.cy() - 13.0, uw, 26.0);
                card_text(g, r, ic::INFO, &format!("Veyra {}", update::CURRENT), "Updates itself", ub.x - r.x - 10.0);
                let hv = ui.hover(ub);
                if self.upd_avail.is_some() {
                    pill_btn(g, ui, ub, &upd, accent_fg(g), accent_btn(g, GREEN), Act::Opt(Opt::Update));
                } else {
                    pill_btn(g, ui, ub, &upd, btn_fg(g, WHITE), surf(g, hv), Act::Opt(Opt::Update));
                }
                // every color option, as a text file
                let r = card(1);
                let hv = ui.hit(r, Act::Opt(Opt::ThemeFile));
                card_base(g, r, hv);
                card_text(g, r, ic::EDIT, "Theme file", "Every color option, in Notepad", r.w - 40.0);
                g.icon(ic::RIGHT, R::new(r.right() - 30.0, r.y, 20.0, r.h), 10.0, btn_icon(g, T2));
            }
        }
    }

    // -------------------------------------------------------------- colors page

    fn open_colors(&mut self) {
        self.commit_edit();
        theme::ensure_file();
        self.col_text = theme::read_text();
        self.colors_open = true;
        self.col_hsv = theme::to_hsv(self.col_color());
    }

    /// The color now used by what is being colored (its automatic color when it has none).
    fn col_color(&self) -> C {
        let t = &self.gfx.theme;
        match self.col_target {
            COL_ICONS => t.icon.unwrap_or(self.gfx.neutral()),
            COL_TEXT => t.fg,
            0 => t.bg.first().map(|s| s.1).unwrap_or(BLACK),
            1 => t.bg_open.first().map(|s| s.1).unwrap_or(BLACK),
            2 => t.button_accent.unwrap_or(GREEN),
            n => feat(&self.gfx, TABS[(n as usize - 3).min(6)]),
        }
    }

    /// The feature being colored (targets 3 and up), as its index in `TABS`.
    fn col_feature(&self) -> Option<usize> {
        (3..=9).contains(&self.col_target).then(|| self.col_target as usize - 3)
    }

    /// Whether what is being colored follows something else (open island: the background;
    /// highlights: each feature's own color).
    fn col_auto(&self) -> bool {
        match self.col_target {
            1 => !matches!(theme::value(&self.col_text, "expanded_mode").as_deref(), Some("solid") | Some("gradient")),
            2 => self.gfx.theme.button_accent.is_none(),
            COL_ICONS => self.gfx.theme.icon.is_none(),
            COL_TEXT => matches!(theme::value(&self.col_text, "text").as_deref(), None | Some("auto")),
            0 => false,
            n => self.gfx.theme.feature[(n as usize - 3).min(6)].is_none(),
        }
    }

    fn col_apply(&mut self, c: C, save: bool) {
        let hx = theme::hex(c);
        let kv: Vec<(&str, String)> = match self.col_target {
            0 => vec![("mode", "solid".into()), ("solid", hx)],
            1 => vec![("expanded_mode", "solid".into()), ("expanded_solid", hx)],
            2 => vec![("button_accent", hx)],
            COL_ICONS => vec![("icon", hx)],
            COL_TEXT => vec![("text", hx), ("expanded_text", "same".into())],
            n => vec![(theme::FEATURE_KEYS[(n as usize - 3).min(6)], hx)],
        };
        self.col_set(&kv, save);
    }

    /// Changes options in the edited text and shows the result at once; `save` also writes
    /// theme.conf (done when the mouse is let go, not on every move).
    fn col_set(&mut self, kv: &[(&str, String)], save: bool) {
        if self.col_text.is_empty() {
            self.col_text = theme::read_text();
        }
        self.col_text = theme::set_keys(&self.col_text, kv);
        let th = theme::parse(&self.col_text);
        if th != self.gfx.theme {
            self.gfx.set_theme(&th);
        }
        if save && theme::save_text(&self.col_text) {
            // our own write: nothing to reload
            self.theme_mtime = theme::modified();
        }
        self.kick();
    }

    fn col_copy(&mut self) {
        self.edit = None;
        let t = theme::hex(self.col_color());
        let ok = clip::write(self.hwnd, &t);
        self.col_note = Some(if ok { ("Copied", true, Instant::now()) } else { ("Couldn't copy", false, Instant::now()) });
        self.kick();
    }

    /// Uses a color code from the clipboard (#1E4FD8, 1E4FD8, #ABC or rgb(30, 79, 216)).
    fn col_paste(&mut self) {
        self.edit = None;
        match clipboard_text().and_then(|s| parse_color_text(&s)) {
            Some(c) => {
                self.col_hsv = theme::to_hsv(c);
                self.col_apply(c, true);
                self.col_note = Some(("Pasted", true, Instant::now()));
            }
            None => self.col_note = Some(("No color code copied", false, Instant::now())),
        }
        self.kick();
    }

    fn col_preset(&mut self, i: usize) {
        let Some(look) = PRESETS.get(i) else { return };
        self.col_set(&preset_keys(look), true);
        self.col_hsv = theme::to_hsv(self.col_color());
    }

    /// The wheel picks hue (angle) and strength (distance from the middle); the bar picks
    /// brightness, top = brightest.
    fn col_drag(&mut self, a: Act, p: (f32, f32), save: bool) {
        let (mut h, mut s, mut v) = self.col_hsv;
        match a {
            Act::ColWheel(r) => {
                let rad = r.w / 2.0;
                let (dx, dy) = (p.0 - r.cx(), p.1 - r.cy());
                h = (dy.atan2(dx) / std::f32::consts::TAU).rem_euclid(1.0);
                s = ((dx * dx + dy * dy).sqrt() / rad).min(1.0);
                // starting from a very dark color (black, or the near-black automatic text of a
                // light look) the wheel would seem to do nothing: start from a visible brightness
                if v < 0.25 {
                    v = if self.col_target == COL_TEXT && self.gfx.theme.bg_luma_light() { 0.45 } else { 0.7 };
                }
            }
            Act::ColBright(r) => v = (1.0 - (p.1 - r.y) / r.h).clamp(0.0, 1.0),
            _ => return,
        }
        self.col_hsv = (h, s, v);
        self.col_apply(theme::hsv(h, s, v), save);
    }

    /// The wheel image (every hue around, white in the middle) at `px` pixels across.
    fn wheel_bitmap(&self, g: &Gfx, px: u32) -> Option<ID2D1Bitmap> {
        if let Some((n, b)) = &*self.col_wheel.borrow() {
            if *n == px {
                return Some(b.clone());
            }
        }
        let n = px as usize;
        let rad = n as f32 / 2.0;
        let mut buf = vec![0u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                let (dx, dy) = (x as f32 + 0.5 - rad, y as f32 + 0.5 - rad);
                let d = (dx * dx + dy * dy).sqrt();
                // soft one-pixel edge
                let a = (rad - d + 0.5).clamp(0.0, 1.0);
                if a <= 0.0 {
                    continue;
                }
                let c = theme::hsv(dy.atan2(dx) / std::f32::consts::TAU, (d / rad).min(1.0), 1.0);
                let i = (y * n + x) * 4;
                let b = |v: f32| (v * a * 255.0).round() as u8;
                buf[i] = b(c.2);
                buf[i + 1] = b(c.1);
                buf[i + 2] = b(c.0);
                buf[i + 3] = (a * 255.0).round() as u8;
            }
        }
        let bmp = g.create_bitmap(px, px, &buf)?;
        *self.col_wheel.borrow_mut() = Some((px, bmp.clone()));
        Some(bmp)
    }

    /// Name of what can be colored on the Colors page.
    fn col_name(t: u8) -> &'static str {
        match t {
            0 => "Background",
            1 => "When open",
            2 => "Highlights",
            COL_ICONS => "All icons",
            COL_TEXT => "Text",
            3 => "Music",
            4 => "Focus",
            5 => "Water",
            6 => "Shelf",
            7 => "Tasks",
            8 => "Clipboard",
            _ => "Settings",
        }
    }

    /// Settings > Colors. Left: what to color (the island, or the icons) and the ready-made
    /// looks. Right: a card that picks the chosen part's color.
    fn draw_colors(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let th = &g.theme;

        // header: back on the left, the two less common actions on the right
        let top = R::new(c.x, c.y - 6.0, c.w, 26.0);
        let back = R::new(top.x, top.y, 80.0, top.h);
        let hv = ui.hit(back, Act::ColBack);
        g.rrect(back, back.h / 2.0, surf(g, hv));
        g.text("\u{2039}  Settings", back, 12.0, 600, btn_fg(g, WHITE), Al::C);
        let d = 112.0;
        let wheel_px = (d * g.scale).round().max(8.0) as u32;
        let wheel = self.wheel_bitmap(g, wheel_px);
        if let Some(b) = &wheel {
            g.bitmap(b, R::new(back.right() + 12.0, top.cy() - 8.0, 16.0, 16.0), 8.0);
        }
        g.text("Colors", R::new(back.right() + 34.0, top.y, 120.0, top.h), 14.0, 700, WHITE, Al::L);
        let fl = "Advanced\u{2026}";
        let fw = g.measure(fl, 11.5, 600) + 22.0;
        let fb = R::new(top.right() - fw, top.y + 1.0, fw, top.h - 2.0);
        let hv = ui.hover(fb);
        pill_btn(g, ui, fb, fl, btn_fg(g, T2), surf(g, hv), Act::Opt(Opt::ThemeFile));
        let rl = "Reset";
        let rw = g.measure(rl, 11.5, 600) + 22.0;
        let rb = R::new(fb.x - 6.0 - rw, fb.y, rw, fb.h);
        let hv = ui.hover(rb);
        pill_btn(g, ui, rb, rl, btn_fg(g, T2), surf(g, hv), Act::ColReset);

        let lw = 282.0;
        let sy = c.y + 30.0;

        // ---------------------------------------------------------------- what to color
        // Two groups behind a simple switch: the island itself (three named rows), or the icons
        // (eight named tiles). Every choice shows its name and its color now.
        let acc = th.button_accent.unwrap_or(GREEN);
        let icons_group = (3..=COL_ICONS).contains(&self.col_target);
        let seg = R::new(c.x, sy, lw, 28.0);
        g.rrect(seg, 14.0, surf(g, false));
        let half = (seg.w - 6.0) / 2.0;
        for (k, (name, target)) in [("Island", 0u8), ("Icons", COL_ICONS)].into_iter().enumerate() {
            let b = R::new(seg.x + 3.0 + k as f32 * half, seg.y + 3.0, half, seg.h - 6.0);
            let on = icons_group == (k == 1);
            let hv = ui.hit(b, Act::ColTarget(if on { self.col_target } else { target }));
            if on {
                g.rrect(b, 11.0, surf_active(g));
            } else if hv {
                g.rrect(b, 11.0, surf(g, true));
            }
            let w = g.measure(name, 12.0, if on { 700 } else { 500 });
            let gx = b.cx() - (w + 20.0) / 2.0;
            if k == 0 {
                fill_stops(g, R::new(gx, b.cy() - 4.5, 14.0, 9.0), 4.5, &th.bg);
                g.rrect_stroke(R::new(gx + 0.5, b.cy() - 4.0, 13.0, 8.0), 4.0, if on { WHITE.a(0.7) } else { WHITE.a(0.4) }, 1.0, false);
            } else {
                for (j, t) in TABS.iter().take(4).enumerate() {
                    let c = tab_color(g, *t);
                    g.circle(gx + 3.0 + (j % 2) as f32 * 7.0, b.cy() - 3.5 + (j / 2) as f32 * 7.0, 2.6, nudge(if on { c } else { c.a(0.7) }));
                }
            }
            g.text(name, R::new(gx + 20.0, b.y, w + 4.0, b.h), 12.0, if on { 700 } else { 500 }, if on { WHITE } else { T2 }, Al::L);
        }
        let gy = seg.bottom() + 8.0;
        if !icons_group {
            // the island: background, open background, text, highlights (2 x 2, as tall as the icon tiles)
            let open_auto = !matches!(theme::value(&self.col_text, "expanded_mode").as_deref(), Some("solid") | Some("gradient"));
            let text_auto = matches!(theme::value(&self.col_text, "text").as_deref(), None | Some("auto"));
            let tiles: [(u8, C, bool); 4] = [
                (0, th.bg.first().map(|s| s.1).unwrap_or(BLACK), false),
                (1, th.bg_open.first().map(|s| s.1).unwrap_or(BLACK), open_auto),
                (COL_TEXT, th.fg, text_auto),
                (2, acc, th.button_accent.is_none()),
            ];
            let tw = (lw - 3.0) / 2.0;
            let th_ = 61.0;
            for (i, (t, col, auto)) in tiles.iter().enumerate() {
                let r = R::new(c.x + (i % 2) as f32 * (tw + 3.0), gy + (i / 2) as f32 * (th_ + 3.0), tw, th_);
                let on = self.col_target == *t;
                let hv = ui.hit(r, Act::ColTarget(*t));
                g.rrect(r, 11.0, if on { surf_active(g) } else { surf(g, hv) });
                let glow = vivid(*col);
                g.rrect(r, 11.0, glow.a(if on { 0.16 } else if hv { 0.1 } else { 0.06 }));
                if on {
                    g.rrect_stroke(r.inset(0.75, 0.75), 10.25, ring_color(*col), 1.5, false);
                }
                // a tiny picture of the part: the closed pill, the open island, words, a switch
                let sw = R::new(r.x + 11.0, r.cy() - 14.0, 38.0, 28.0);
                let edge = WHITE.a(0.3);
                match *t {
                    0 => {
                        let pill = R::new(sw.x, sw.cy() - 8.0, sw.w, 16.0);
                        fill_stops(g, pill, 8.0, &th.bg);
                        g.rrect_stroke(pill.inset(0.5, 0.5), 7.5, edge, 1.0, false);
                        g.circle(pill.x + 8.0, pill.cy(), 2.2, nudge(th.fg.a(0.8)));
                        g.rrect(R::new(pill.x + 13.0, pill.cy() - 1.2, 16.0, 2.4), 1.2, nudge(th.fg.a(0.55)));
                    }
                    1 => {
                        fill_stops(g, sw, 8.0, &th.bg_open);
                        g.rrect_stroke(sw.inset(0.5, 0.5), 7.5, edge, 1.0, false);
                        for j in 0..3 {
                            g.circle(sw.x + 9.0 + j as f32 * 7.0, sw.y + 8.0, 1.8, nudge(th.fg.a(0.7)));
                        }
                        g.rrect(R::new(sw.x + 6.0, sw.y + 15.0, sw.w - 12.0, 2.4), 1.2, nudge(th.fg.a(0.45)));
                        g.rrect(R::new(sw.x + 6.0, sw.y + 20.0, sw.w - 20.0, 2.4), 1.2, nudge(th.fg.a(0.3)));
                    }
                    COL_TEXT => {
                        // the text color on the island's background
                        fill_stops(g, sw, 8.0, &th.bg);
                        g.rrect_stroke(sw.inset(0.5, 0.5), 7.5, edge, 1.0, false);
                        g.text("Aa", sw, 13.0, 700, nudge(*col), Al::C);
                    }
                    _ => {
                        let track = R::new(sw.x + 2.0, sw.cy() - 9.0, sw.w - 4.0, 18.0);
                        g.rrect(track, 9.0, nudge(*col));
                        g.circle(track.right() - 9.0, track.cy(), 6.0, accent_fg(g));
                    }
                }
                let tx = sw.right() + 9.0;
                g.text(Self::col_name(*t), R::new(tx, r.cy() - 17.0, r.right() - tx - 6.0, 17.0), 12.5, 600, WHITE, Al::L);
                let code = if *auto { "Auto".to_string() } else { theme::hex(*col) };
                g.text(&code, R::new(tx, r.cy() + 1.0, r.right() - tx - 6.0, 15.0), 10.5, 600, T3, Al::L);
            }
        } else {
            // the icons: every regular icon, then each feature
            let cols = 4usize;
            let tw = (lw - 3.0 * (cols as f32 - 1.0)) / cols as f32;
            let th_ = 61.0;
            for k in 0..8usize {
                let t = if k == 0 { COL_ICONS } else { 3 + (k - 1) as u8 };
                let r = R::new(c.x + (k % cols) as f32 * (tw + 3.0), gy + (k / cols) as f32 * (th_ + 3.0), tw, th_);
                let on = self.col_target == t;
                let hv = ui.hit(r, Act::ColTarget(t));
                g.rrect(r, 11.0, if on { surf_active(g) } else { surf(g, hv) });
                let own = if k == 0 { th.icon.unwrap_or(g.neutral()) } else { tab_color(g, TABS[k - 1]) };
                if on {
                    g.rrect(r, 11.0, vivid(own).a(0.12));
                    g.rrect_stroke(r.inset(0.75, 0.75), 10.25, ring_color(own), 1.5, false);
                }
                let ir = R::new(r.x, r.y + 9.0, r.w, 24.0);
                g.circle(ir.cx(), ir.cy(), 14.0, vivid(own).a(if on { 0.26 } else if hv { 0.2 } else { 0.13 }));
                if k == 0 {
                    g.icon(ic::PLAY, ir, 15.0, nudge(th.icon.unwrap_or(g.neutral())));
                } else {
                    let tab = TABS[k - 1];
                    draw_tab_icon_sized(g, tab, ir, tab_color(g, tab), 15.0);
                }
                let name = if k == 0 { "All" } else { Self::col_name(t) };
                g.text(name, R::new(r.x, r.y + 36.0, r.w, 16.0), 11.0, if on { 700 } else { 500 }, if on { WHITE } else { T2 }, Al::C);
            }
        }

        // ---------------------------------------------------------------- ready-made looks
        let open_differs = th.bg != th.bg_open;
        let ly = sy + 176.0;
        let cw = lw / 4.0;
        let mut named: Option<&str> = None;
        for (i, look) in PRESETS.iter().enumerate() {
            let cell = R::new(c.x + (i % 4) as f32 * cw, ly + 20.0 + (i / 4) as f32 * 52.0, cw, 50.0);
            let hv = ui.hit(cell, Act::ColPreset(i));
            let stops = preset_bg(look.bg);
            let picked = th.bg == stops && !open_differs;
            if hv || (picked && named.is_none()) {
                named = Some(look.name);
            }
            let thumb = R::new(cell.x + 5.0, cell.y, cell.w - 10.0, 30.0);
            fill_stops(g, thumb, 9.0, &stops);
            // the look's colors: buttons, icons, softer icons
            let button = look.extra.iter().find(|(k, _)| *k == "button_accent").map(|(_, v)| *v).unwrap_or(look.main);
            let dots: Vec<C> = [button, look.main, look.soft].iter().filter_map(|h| theme::color(h)).collect();
            let dots = if dots.is_empty() { vec![GREEN, C::hex(0x4DB8FF), C::hex(0xFF7A59)] } else { dots };
            for (k, d) in dots.iter().enumerate() {
                g.circle(thumb.x + 10.0 + k as f32 * 9.0, thumb.bottom() - 9.0, 3.2, nudge(*d));
            }
            if picked {
                g.rrect_stroke(thumb.inset(-2.5, -2.5), 11.5, WHITE.a(0.95), 1.6, false);
                g.circle(thumb.right() - 8.0, thumb.y + 8.0, 6.0, nudge(acc));
                g.icon(ic::CHECK, R::new(thumb.right() - 14.0, thumb.y + 2.0, 12.0, 12.0), 7.0, accent_fg(g));
            } else if hv {
                g.rrect_stroke(thumb.inset(-2.5, -2.5), 11.5, WHITE.a(0.5), 1.2, false);
            } else {
                g.rrect_stroke(thumb.inset(0.5, 0.5), 8.5, WHITE.a(0.14), 1.0, false);
            }
            g.text(look.name, R::new(cell.x, thumb.bottom() + 2.0, cell.w, 15.0), 10.5, if picked { 700 } else { 500 }, if picked || hv { WHITE } else { T2 }, Al::C);
        }
        let head = "Ready-made looks";
        g.text(head, R::new(c.x + 2.0, ly, lw, 16.0), 11.5, 600, T2, Al::L);
        if let Some(n) = named {
            let hw = g.measure(head, 11.5, 600);
            g.text(&format!("\u{00B7}  {n}"), R::new(c.x + hw + 8.0, ly, 120.0, 16.0), 11.5, 500, T3, Al::L);
        }

        // ---------------------------------------------------------------- the color card
        let card = R::new(c.x + lw + 14.0, sy, c.right() - (c.x + lw + 14.0), ly + 124.0 - sy);
        let cur = self.col_color();
        g.rrect(card, 16.0, WHITE.a(0.05));
        let lit = vivid(cur);
        g.rrect_vgrad(R::new(card.x, card.y, card.w, 130.0), 16.0, lit.a(0.22), lit.a(0.0));
        g.rrect_stroke(card.inset(0.5, 0.5), 15.5, lit.a(0.28), 1.0, false);
        let ix = card.x + 14.0;
        let iw = card.w - 28.0;
        let auto = self.col_auto();
        let feature = self.col_feature().map(|i| TABS[i]);
        let ty = card.y + 12.0;
        match feature {
            Some(t) => draw_tab_icon(g, t, R::new(ix - 4.0, ty, 22.0, 18.0), tab_color(g, t)),
            None if self.col_target == COL_ICONS => g.icon(ic::PLAY, R::new(ix - 4.0, ty, 22.0, 18.0), 12.0, nudge(th.icon.unwrap_or(g.neutral()))),
            None => {
                g.circle(ix + 6.0, ty + 9.0, 6.0, nudge(cur));
                g.circle_stroke(ix + 6.0, ty + 9.0, 6.0, WHITE.a(0.35), 1.0);
            }
        }
        g.text(Self::col_name(self.col_target), R::new(ix + 20.0, ty, iw - 20.0, 18.0), 12.5, 700, WHITE, Al::L);
        let hint = match (self.col_target, feature) {
            (0, _) if open_differs => "The island while it is closed.",
            (0, _) if matches!(theme::value(&self.col_text, "text").as_deref(), None | Some("auto")) => "Text stays readable by itself.",
            (0, _) => "The island while it is closed.",
            (1, _) if auto => "Same as Background for now.",
            (1, _) => "The island while it is open.",
            (2, _) => "Main buttons and switches.",
            (COL_TEXT, _) if auto => "Light or dark, whichever reads.",
            (COL_TEXT, _) => "All the words in the island.",
            (COL_ICONS, _) => "Play, next, tabs not open\u{2026}",
            (_, Some(Tab::Media)) => "The Music tab icon.",
            (_, Some(Tab::Focus)) => "Tab icon, timer and notices.",
            (_, Some(Tab::Water)) => "Icon, buttons and reminders.",
            (_, Some(Tab::Shelf)) => "Icon, tiles and file types.",
            (_, Some(Tab::Tasks)) => "Icon, buttons and reminders.",
            _ => "Tab icon and buttons.",
        };
        g.text(hint, R::new(ix, ty + 18.0, iw, 16.0), 10.5, 500, T3, Al::L);

        // wheel (color and strength) with the brightness bar beside it
        let wy = ty + 42.0;
        let (gap, bw) = (12.0, 14.0);
        let wx = ix + (iw - (d + gap + bw)) / 2.0;
        let wr = R::new(wx, wy, d, d);
        ui.hit(wr, Act::ColWheel(wr));
        g.circle(wr.cx(), wr.cy() + 2.0, d / 2.0 + 3.0, C(0.0, 0.0, 0.0, 0.22));
        if let Some(b) = &wheel {
            g.bitmap(b, wr, d / 2.0);
        }
        g.circle_stroke(wr.cx(), wr.cy(), d / 2.0 + 0.5, WHITE.a(0.18), 1.0);
        let (h, s, v) = self.col_hsv;
        let snow = C(0.999, 0.999, 0.999, 1.0);
        if !auto {
            let ang = h * std::f32::consts::TAU;
            let (mx, my) = (wr.cx() + ang.cos() * s * d / 2.0, wr.cy() + ang.sin() * s * d / 2.0);
            g.circle(mx, my, 8.5, C(0.0, 0.0, 0.0, 0.35));
            g.circle(mx, my, 6.0, nudge(theme::hsv(h, s, v.max(0.15))));
            g.circle_stroke(mx, my, 6.5, snow, 2.0);
        }
        let br = R::new(wr.right() + gap, wy, bw, d);
        ui.hit(R::new(br.x - 5.0, br.y - 4.0, br.w + 10.0, br.h + 8.0), Act::ColBright(br));
        g.push_rr(br, bw / 2.0, 1.0);
        let n = 40;
        for k in 0..n {
            let col = theme::hsv(h, s, 1.0 - k as f32 / (n - 1) as f32);
            g.rect(R::new(br.x, br.y + br.h * k as f32 / n as f32, br.w, br.h / n as f32 + 0.6), nudge(col));
        }
        g.pop();
        g.rrect_stroke(br, bw / 2.0, WHITE.a(0.3), 1.0, false);
        if !auto {
            let ky = br.y + (1.0 - v) * br.h;
            g.rrect(R::new(br.x - 3.0, ky - 3.5, br.w + 6.0, 7.0), 3.5, C(0.0, 0.0, 0.0, 0.35));
            g.rrect_stroke(R::new(br.x - 2.0, ky - 2.5, br.w + 4.0, 5.0), 2.5, snow, 1.8, false);
        }

        // the color's code (click to type one), copy and paste
        let ry = wy + d + 12.0;
        let ib = 24.0;
        let hb = R::new(ix, ry, iw - 2.0 * (ib + 4.0), 26.0);
        let editing = self.edit.as_ref().filter(|e| e.field == Field::Hex);
        let hv = ui.hit(hb, Act::Edit(Field::Hex));
        g.rrect(hb, 9.0, if editing.is_some() { surf_active(g) } else { surf(g, hv) });
        g.circle(hb.x + 11.0, hb.cy(), 5.5, nudge(cur));
        g.circle_stroke(hb.x + 11.0, hb.cy(), 5.5, WHITE.a(0.35), 1.0);
        let tr = R::new(hb.x + 19.0, hb.y, hb.w - 21.0, hb.h);
        let note = self.col_note.filter(|(_, _, at)| at.elapsed() < Duration::from_millis(1600));
        match (editing, note) {
            (Some(e), _) => {
                let t = format!("#{}|", e.buf.trim_start_matches('#').to_uppercase());
                g.text(&t, tr, 12.5, 600, WHITE, Al::C);
            }
            (None, Some((msg, good, _))) => g.text(msg, tr, 11.0, 600, if good { GREEN } else { RED }, Al::C),
            (None, None) if auto => g.text("Automatic", tr, 11.5, 600, T2, Al::C),
            (None, None) => g.text(&theme::hex(cur), tr, 12.0, 600, WHITE, Al::C),
        }
        let cb = R::new(hb.right() + 4.0, ry, ib, 26.0);
        icon_btn(g, ui, cb, ic::COPY, 12.0, T2, Act::ColCopy, true);
        let pb = R::new(cb.right() + 4.0, ry, ib, 26.0);
        icon_btn(g, ui, pb, ic::PASTE, 12.0, T2, Act::ColPaste, true);

        // quick colors: one click, no wheel needed (deep tones for backgrounds, bright ones else)
        let quick: &[u32; 8] = match self.col_target {
            0 | 1 => &QUICK_BG,
            COL_TEXT if th.bg_luma_light() => &QUICK_TEXT_LIGHT,
            COL_TEXT => &QUICK_TEXT,
            _ => &QUICK_ACCENT,
        };
        g.text("Quick colors", R::new(ix, ry + 34.0, iw, 16.0), 10.5, 600, T3, Al::L);
        let qy = ry + 60.0;
        let step = iw / 8.0;
        for (k, hx) in quick.iter().enumerate() {
            let qc = C::hex(*hx);
            let (cx, cy) = (ix + step * (k as f32 + 0.5), qy);
            let qr = R::new(cx - step / 2.0, cy - 10.0, step, 20.0);
            let hv = ui.hit(qr, Act::ColSwatch(*hx));
            let picked = !auto && theme::hex(cur) == theme::hex(qc);
            g.circle(cx, cy, if hv { 7.5 } else { 6.5 }, nudge(qc));
            if picked {
                g.circle_stroke(cx, cy, 9.5, WHITE.a(0.95), 1.5);
            } else {
                g.circle_stroke(cx, cy, if hv { 7.5 } else { 6.5 }, WHITE.a(0.25), 1.0);
            }
        }

        if self.col_target != 0 {
            let al = match self.col_target {
                1 => "Same as background",
                2 => "Automatic colors",
                COL_ICONS => "Automatic",
                COL_TEXT => "Automatic",
                _ => "Original color",
            };
            let ab = R::new(ix, card.bottom() - 36.0, iw, 24.0);
            let hv = ui.hit(ab, Act::ColAuto);
            g.rrect(ab, 12.0, if auto { surf_active(g) } else { surf(g, hv) });
            if auto {
                let tw = g.measure(al, 11.5, 600);
                g.icon(ic::CHECK, R::new(ab.cx() - tw / 2.0 - 16.0, ab.y, 12.0, ab.h), 9.0, WHITE);
            }
            g.text(al, ab, 11.5, if auto { 600 } else { 500 }, if auto { WHITE } else { T2 }, Al::C);
        }
    }

    /// Mini lyrics are on and this track's synced lyrics are here.
    fn mini_on(&self) -> bool {
        self.data.settings.mini_lyrics
            && matches!(&self.lyr, Lyr::Found(_))
            && self.media.as_ref().map(|m| m.key == self.lyr_key).unwrap_or(false)
    }

    /// Width of the lyric line in the closed island: fits the song's longest line (within
    /// limits), worked out once per track so the island keeps one size for the whole song.
    fn mini_text_w(&self) -> f32 {
        // no lyrics (yet): the last width, while the pill turns back into the clock
        let Lyr::Found(v) = &self.lyr else { return self.mini_last_w.get() };
        let w = match self.mini_w.get() {
            Some((k, w)) if k == self.lyr_key => w,
            _ => {
                let longest = v.iter().map(|l| self.gfx.measure(&l.text, MINI_SIZE, 600)).fold(0.0f32, f32::max);
                let w = (longest + 8.0).clamp(MINI_MIN_W, MINI_MAX_W);
                self.mini_w.set(Some((self.lyr_key, w)));
                w
            }
        };
        self.mini_last_w.set(w);
        w
    }

    /// clock (0) -> lyric line (1) in the closed pill, eased in and out
    fn mini_k(&self) -> f32 {
        let p = self.mini_p.clamp(0.0, 1.0);
        p * p * p * (p * (p * 6.0 - 15.0) + 10.0)
    }

    /// How far a morph has come, 0..1, measured by the island's height.
    fn morph_p(&self, m: &Morph) -> f32 {
        let span = self.sh.t - m.h0;
        if span.abs() < 1.0 {
            smooth(self.view_age / 0.35)
        } else {
            ((self.sh.x - m.h0) / span).clamp(0.0, 1.0)
        }
    }

    /// The nearest unfinished task due within the next few minutes (or just overdue).
    fn task_soon(&self) -> Option<(&Task, i64)> {
        let n = now().secs;
        self.data
            .tasks
            .iter()
            .filter(|t| !t.done)
            .filter_map(|t| t.due.map(|d| (t, d - n)))
            .filter(|(_, left)| *left <= TASK_SOON_MIN * 60 && *left > -10 * 60)
            .min_by_key(|(_, left)| *left)
    }

    fn task_line(&self) -> Option<(String, String, bool)> {
        let (t, left) = self.task_soon()?;
        let due = if left <= 0 {
            "now".to_string()
        } else {
            format!("in {}m", (left + 59) / 60)
        };
        Some((t.text.clone(), due, left <= 0))
    }

    fn compact_width(&self, live: Live) -> f32 {
        let g = &self.gfx;
        match live {
            Live::Idle => IDLE_W,
            // art | clock or lyric line | equalizer — must mirror draw_compact
            Live::Media | Live::Lyrics => {
                let clock = 7.0 + (PILL_H - 10.0) + 6.0 + g.measure_clock("88:88", CLOCK_SIZE, 600) + 24.0 + 6.0 + 18.0 + 14.0;
                let k = self.mini_k();
                if k <= 0.0 {
                    return clock;
                }
                let lyric = 7.0 + (PILL_H - 10.0) + 10.0 + self.mini_text_w() + 10.0 + 18.0 + 14.0;
                clock + (lyric - clock) * k
            }
            Live::Pomo => {
                let label = if self.pomo.mode == PMode::Break { "Break" } else { "Focus" };
                12.0 + 16.0 + 6.0 + g.measure(label, 12.5, 600) + 22.0 + 16.0 + 6.0 + g.measure("88:88", 13.5, 600) + 14.0
            }
            Live::Task => {
                let (text, due, _) = self.task_line().unwrap_or_default();
                12.0 + 16.0 + 8.0 + g.measure(&text, 12.5, 500).clamp(30.0, 170.0) + 10.0 + g.measure(&due, 12.0, 600) + 14.0
            }
        }
    }


    // -------------------------------------------------------------- frame

    pub fn frame(&mut self) {
        let n = Instant::now();
        let dt = (n - self.last_frame).as_secs_f32().min(1.0 / 30.0);
        self.last_frame = n;
        self.anim_stat.1 += 1;
        // clock <-> lyric line: eased over MINI_SECS in the closed pill; anywhere else it is
        // simply where it should be (the island closes straight into the right one)
        let mini_t = if self.mini_on() { 1.0 } else { 0.0 };
        // (the line shows neither: the pill drops out of it already showing the right one)
        let pill = matches!(self.view, ViewKey::Compact(_));
        let mini_moving = pill && (self.mini_p - mini_t).abs() > 0.0001;
        if !pill {
            self.mini_p = mini_t;
        } else if mini_moving {
            let step = dt / MINI_SECS;
            self.mini_p = if mini_t > self.mini_p { (self.mini_p + step).min(1.0) } else { (self.mini_p - step).max(0.0) };
        }
        let (tw, th, tr, key) = self.target();
        let hover_still = self.step_hover(n, dt, key);
        self.view_wh = Some((tw, th));
        if key != self.view {
            self.anim_view_change = true;
            match (self.view, key) {
                // switching feature while open: slide instead of fade
                (ViewKey::Expanded(a, al), ViewKey::Expanded(b, _)) if a != b && self.view_age > 0.05 => {
                    let idx = |t: Tab| TABS.iter().position(|x| *x == t).unwrap_or(0) as f32;
                    self.slide = Some(Slide {
                        from_tab: a,
                        from_lyr: al,
                        from_w: self.sw.t,
                        from_h: self.sh.t,
                        dir: (idx(b) - idx(a)).signum(),
                        t: 0.0,
                        sub: false,
                    });
                }
                // Settings <-> Colors: the next page glides in from the right (back: from the left)
                (ViewKey::Expanded(Tab::Settings, al), ViewKey::Expanded(Tab::Settings, bl)) if al != bl && self.view_age > 0.05 => {
                    self.slide = Some(Slide {
                        from_tab: Tab::Settings,
                        from_lyr: al,
                        from_w: self.sw.t,
                        from_h: self.sh.t,
                        dir: if bl { 1.0 } else { -1.0 },
                        t: 0.0,
                        sub: true,
                    });
                }
                _ => {
                    self.slide = None;
                    self.view_age = 0.0;
                }
            }
            if key == ViewKey::Line {
                self.line_from_pill = matches!(self.view, ViewKey::Compact(_));
            }
            // open <-> closed with music: the artwork travels to its new place
            let media_view = |v: ViewKey| matches!(v, ViewKey::Expanded(Tab::Media, _) | ViewKey::Compact(Live::Media));
            self.morph = match self.art_last.get() {
                Some((from, from_rad)) if media_view(self.view) && media_view(key) && self.media.is_some() && self.leaving.is_none() => {
                    Some(Morph { from, from_rad, h0: self.sh.x, old: self.view, old_w: self.sw.t, old_h: self.sh.t })
                }
                _ => None,
            };
            self.pill_from_line = matches!(key, ViewKey::Compact(_)) && self.view == ViewKey::Line && self.sh.x < PILL_H;
            self.view = key;
        }
        if let Some(sl) = &mut self.slide {
            // a page inside the tab takes a little longer: it cross-fades as it glides
            sl.t += dt / if sl.sub { SLIDE_SECS * 1.25 } else { SLIDE_SECS };
            if sl.t >= 1.0 {
                self.slide = None;
            }
        }
        self.sw.t = tw;
        self.sh.t = th;
        self.sr.t = tr;
        // line mode, between the line and the pill (not a panel or notice closing to the line)
        let line_pill = self.data.settings.line_mode
            && self.leaving.is_none()
            && match self.view {
                ViewKey::Line => self.line_from_pill,
                ViewKey::Compact(_) => true,
                _ => false,
            };
        let mut rem = dt;
        while rem > 0.0 {
            let s = rem.min(1.0 / 240.0);
            if self.morph.is_some() {
                self.sw.step_fast(s, MORPH_W);
                self.sh.step_fast(s, MORPH_W);
                self.sr.step_fast(s, MORPH_W);
            } else if line_pill {
                let (ww, hw) = if self.view == ViewKey::Line {
                    (LINE_SPRING_W, LINE_RISE)
                } else {
                    (LINE_DROP_WIDEN, LINE_DROP_FALL)
                };
                self.sw.step_fast(s, ww);
                self.sh.step_fast(s, hw);
                self.sr.step_fast(s, hw);
            } else {
                self.sw.step(s);
                self.sh.step(s);
                self.sr.step(s);
            }
            rem -= s;
        }
        // the clock <-> lyrics morph moves the width itself, eased: the shape follows it exactly
        if mini_moving && matches!(self.view, ViewKey::Compact(_)) && self.morph.is_none() && self.view_age > 0.35 {
            self.sw.x = self.sw.t;
            self.sw.v = 0.0;
        }
        if self.leaving.is_some() {
            self.morph = None;
        }
        if let Some(m) = self.morph {
            if self.morph_p(&m) >= 0.999 && self.sw.done() && self.sh.done() {
                self.morph = None;
            }
        }
        self.view_age += dt;
        // closed ↔ open colors: ease toward the open colors while expanded
        let open_t = if matches!(self.view, ViewKey::Expanded(..)) { 1.0 } else { 0.0 };
        let open_moving = (open_t - self.open_k).abs() > 0.003;
        self.open_k = if open_moving { self.open_k + (open_t - self.open_k) * (1.0 - (-dt * 11.0).exp()) } else { open_t };
        self.gfx.set_open(self.open_k);
        // shelf: slide toward the scroll position (ease-out)
        self.shelf_scroll = self.shelf_scroll.min(self.shelf_max());
        let target = self.shelf_scroll as f32;
        let d = target - self.shelf_x;
        let shelf_moving = d.abs() > 0.002;
        if shelf_moving {
            self.shelf_x += d * (1.0 - (-dt * 13.0).exp());
        } else {
            self.shelf_x = target;
        }
        let mut lyr_moving = false;
        if let Some(c) = self.cur_line() {
            let t = c as f32;
            let d = t - self.lyr_scroll;
            if d.abs() > 6.0 {
                self.lyr_scroll = t;
            } else {
                self.lyr_scroll += d * (dt * 10.0).min(1.0);
            }
            lyr_moving = d.abs() > 0.004;
        }
        if let Some((what, since, _)) = self.leaving {
            if self.sh.x <= 0.5 || since.elapsed() >= LEAVE_MAX {
                self.render();
                self.finish_leave(what);
                return;
            }
        }
        let dump = std::env::var("VEYRA_DUMP").ok();
        if !self.hidden || dump.is_some() {
            self.render();
        }
        // testing aid: VEYRA_DUMP_SEQ=<folder> saves every frame (f000.bmp, ...) for ~2.5 s, then quits
        if let Ok(dir) = std::env::var("VEYRA_DUMP_SEQ") {
            let i = self.anim_stat_seq;
            self.anim_stat_seq += 1;
            if !self.hidden {
                self.gfx.dump_bmp(&format!("{dir}/f{i:03}.bmp"), self.win.2, self.win.3);
            }
            if self.t0.elapsed() > Duration::from_millis(2500) {
                crate::quit_now();
            }
        }
        // testing aid: with VEYRA_DEMO + VEYRA_DUMP=<file.bmp>, save the settled frame and quit
        if let Some(path) = dump {
            if self.view_age > 0.3 && self.slide.is_none() && self.sw.done() && self.sh.done() {
                self.gfx.dump_bmp(&path, self.win.2, self.win.3);
                crate::quit_now();
            }
        }
        let settled = self.sw.done()
            && self.sh.done()
            && self.sr.done()
            && self.view_age > 0.35
            && self.slide.is_none()
            && !lyr_moving
            && !shelf_moving
            && !open_moving
            && hover_still
            && !mini_moving
            && self.morph.is_none()
            && !self.marquee_on.get();
        if settled {
            self.sw.snap();
            self.sh.snap();
            self.sr.snap();
            // the animation's layer bitmaps are not needed while resting: let them go now
            self.gfx.release_layers();
            self.anim_on = false;
            crate::vsync::run(false);
            let (t0, frames) = self.anim_stat;
            let view_changed = std::mem::take(&mut self.anim_view_change);
            let secs = n.duration_since(t0).as_secs_f32();
            if frames > 10 {
                dbg_log(&format!("anim: {frames} frames in {:.0} ms ({:.0} fps)", secs * 1000.0, frames as f32 / secs));
                // a real animation (hover, notice, collapse) paged things back in: trim again
                // a little after the island comes to rest (single redraws don't count)
                // (a hover alone doesn't count: the next hover would stutter paging it back in)
                if view_changed && !self.expanded && self.toast.is_none() && self.leaving.is_none() {
                    self.trim_at = Some(n + Duration::from_secs(3));
                }
            }
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_ANIM);
            }
        }
    }

    /// Advance the hover spring. Returns true once hover is at rest (nothing pending).
    fn step_hover(&mut self, n: Instant, dt: f32, key: ViewKey) -> bool {
        let compact = matches!(key, ViewKey::Compact(_) | ViewKey::Line) && self.leaving.is_none() && !self.hidden;
        // in line mode, stay open a little longer once the mouse leaves, so it does not flicker
        let out_delay = if self.data.settings.line_mode { LINE_OUT } else { HOVER_OUT };
        // just closed in line mode: the panel is still shrinking under the cursor, no hover yet
        let settling = self.no_hover_until.map(|t| n < t).unwrap_or(false);
        // the island's own menu took the mouse away: it stays as it is until the menu is gone
        let raw = compact && (self.menu_hold || (!settling && (self.mouse.is_some() || self.drag_hover)));
        // a cursor waiting out the settling time: keep frames coming so it is noticed afterwards
        let mut pending = compact && settling && self.mouse.is_some();
        if !compact {
            self.hover_on = false;
        } else if raw && !self.hover_on {
            // dragging files onto the island reacts at once
            let since = self.hover_in.map(|t| n.duration_since(t)).unwrap_or(HOVER_IN);
            if self.drag_hover || since >= HOVER_IN {
                self.hover_on = true;
            } else {
                pending = true;
            }
        } else if !raw && self.hover_on {
            let since = self.hover_out.map(|t| n.duration_since(t)).unwrap_or(out_delay);
            if since >= out_delay {
                self.hover_on = false;
            } else {
                pending = true;
            }
        }
        let t = if self.hover_on { 1.0 } else { 0.0 };
        let w = if self.data.settings.line_mode {
            LINE_SPRING_W
        } else if self.hover_on {
            HOVER_W_IN
        } else {
            HOVER_W_OUT
        };
        let mut rem = dt;
        while rem > 0.0 {
            let s = rem.min(1.0 / 240.0);
            let a = w * w * (t - self.hov) - 2.0 * w * self.hov_v;
            self.hov_v += a * s;
            self.hov += self.hov_v * s;
            rem -= s;
        }
        let rest = (self.hov - t).abs() < 0.004 && self.hov_v.abs() < 0.05;
        if rest {
            self.hov = t;
            self.hov_v = 0.0;
        }
        rest && !pending
    }

    fn render(&mut self) {
        self.sync_thumbs();
        let s = self.gfx.scale;
        let w = (self.sw.x + HOVER_W * self.hov).max(1.0);
        let h = (self.sh.x + HOVER_H * self.hov).max(0.0);
        // extra room on both sides for the concave "shoulders" where the notch meets the screen edge
        let wpx = ((w + 2.0 * SHOULDER) * s).ceil() as i32 + 2;
        // Line mode: an invisible strip at the very top, the line plus about 1 cm each side,
        // catches the mouse. It stays while the pill is open too: a cursor beside a narrow pill
        // that opened it is still "on" Veyra, so the pill does not close under it, bring the
        // line (and its strip) back under the cursor and open again, over and over.
        let strip = self.data.settings.line_mode
            && matches!(self.view, ViewKey::Line | ViewKey::Compact(_))
            && self.leaving.is_none()
            && !self.hidden;
        let strip_w = LINE_W + 2.0 * LINE_HIT_PAD;
        let wpx = if strip { wpx.max((strip_w * s).ceil() as i32 + 2) } else { wpx };
        let hpx = (h * s).ceil() as i32 + 1;
        let left = self.mon_cx - wpx / 2;
        let top = self.mon_top;
        self.screen_cx = (wpx / 2) as f32 / s;
        if !self.gfx.begin(wpx, hpx) {
            return;
        }
        // Whole pixels for the island's left and right edges: a side falling between pixels
        // is drawn half transparent, which shows as a dark line on light island colors.
        let wi = ((w * s).round() as i32).clamp(1, wpx);
        let xi = (wpx - wi) / 2;
        let island = R::new(xi as f32 / s, 0.0, wi as f32 / s, h);
        let mut ui = Ui { hits: Vec::with_capacity(32), mouse: self.mouse };
        if strip {
            // alpha 1/255: invisible, but the window still receives the mouse there
            let sh = (LINE_H + 1.0).min(hpx as f32 / s);
            let sx = (wpx as f32 / s - strip_w) / 2.0;
            self.gfx.rect(R::new(sx, 0.0, strip_w, sh), C(0.0, 0.0, 0.0, 1.0 / 255.0));
        }
        self.marquee_on.set(false);
        self.draw(&self.gfx, &mut ui, island);
        if !self.marquee_on.get() {
            // nothing long under the mouse any more: the next one starts from its beginning
            self.marquee_at.set(None);
        }
        self.gfx.end(self.hwnd, left, top, wpx, hpx);
        self.win = (left, top, wpx, hpx);
        self.isl_px = (
            left + (island.x * s) as i32,
            top,
            (island.w * s).ceil() as i32 + 1,
            hpx,
        );
        self.hits = ui.hits;
    }

    fn draw(&self, g: &Gfx, ui: &mut Ui, isl: R) {
        if isl.h < 0.5 {
            return;
        }
        let rad = self.sr.x.clamp(0.0, isl.h / 2.0);
        // Attached to the top edge like a notch: extend the shape upward by its radius so
        // only the bottom corners are rounded, then add the concave shoulders.
        let shape = R::new(isl.x, isl.y - rad, isl.w, isl.h + rad);
        let (top, bottom) = (isl.y, isl.y + isl.h);
        self.shape_r.set(isl);
        g.rrect_bg(shape, rad, top, bottom);
        let f = SHOULDER.min(isl.h * 0.5);
        g.shoulder_bg(isl.x, isl.y, f, true, top, bottom);
        g.shoulder_bg(isl.right(), isl.y, f, false, top, bottom);
        // Line mode: the pill comes down out of (and goes back up into) the 5 px line, its content
        // sliding with the shape's lower edge and clipped by it, as if it was above the screen.
        let sliding = self.data.settings.line_mode
            && match self.view {
                ViewKey::Line => self.line_from_pill,
                ViewKey::Compact(_) => self.pill_from_line,
                _ => false,
            };
        // Whole pixels, and already home while the shape creeps its last pixel: the text must not
        // change its blur on the way or hop into place when the spring finally snaps to rest.
        let px = |v: f32| (v * self.gfx.scale).round() / self.gfx.scale;
        let drop_y = if sliding {
            px(-PILL_H * (1.0 - ((self.sh.x - LINE_H) / (PILL_H - 1.0 - LINE_H)).clamp(0.0, 1.0)))
        } else {
            0.0
        };
        let mut alpha = if self.view == ViewKey::Line {
            // nothing of the pill left to show at the line; the open island or a notice closing
            // straight to the line must not flash the pill's content on the way
            if sliding && drop_y > -PILL_H + 0.01 {
                1.0
            } else {
                0.0
            }
        } else if sliding {
            1.0
        } else if self.data.settings.line_mode && matches!(self.view, ViewKey::Compact(_)) {
            smooth(self.view_age / 0.2)
        } else {
            smooth((self.view_age - 0.05) / 0.22)
        };
        if let Some((_, since, _)) = self.leaving {
            // content goes first, then the black shape follows it into the edge
            alpha *= 1.0 - smooth(since.elapsed().as_secs_f32() / 0.12);
        }
        let morph = self.morph.filter(|_| self.leaving.is_none());
        self.art_last.set(None);
        // a morph still needs to know where the new view puts the artwork
        if alpha <= 0.01 && morph.is_none() {
            return;
        }
        // The size that came with `view`. Asking target() again here could answer for a newer
        // view: when the hover ends mid-frame it already says "line" while `view` is still the
        // pill, and the pill's content would be squeezed into a 5 px box for one frame (text
        // jumping up and cut off just before the island closes).
        let (tw, th) = self.view_wh.unwrap_or_else(|| {
            let (w, h, _, _) = self.target();
            (w, h)
        });
        // Hovered, the pill grows a little taller: its content moves down by half of that, so it
        // stays in the middle. It glides (no whole-pixel steps on the way) straight to the whole-
        // pixel offset it rests at, so nothing hops once the hover settles and the text at rest
        // stays sharp (rounding only at the end moved it half a pixel late at 125 % scaling)
        let hov_dy = if matches!(self.view, ViewKey::Compact(_)) {
            px(HOVER_H / 2.0) * self.hov.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let c = match self.view {
            ViewKey::Line => {
                // still rising from the pill: lay its content out at the pill's size
                let (live, sec) = self.lives();
                let cw = self.compact_width(live) + self.segment_width(sec);
                R::new(px(self.screen_cx - cw / 2.0), isl.y + drop_y, cw, PILL_H)
            }
            // the shape's width (hover) moves its center by a pixel now and then: not the content
            ViewKey::Compact(_) if self.data.settings.line_mode => R::new(px(self.screen_cx - tw / 2.0), isl.y + drop_y + hov_dy, tw, th),
            // centered on the screen, not on the shape: the shape grows by whole pixels, so its
            // center wobbles by half a pixel from frame to frame while hovering, and the content
            // would shake with it
            ViewKey::Compact(_) => R::new(px(self.screen_cx - tw / 2.0), isl.y + hov_dy, tw, th),
            _ => R::new(isl.cx() - tw / 2.0, isl.y, tw, th),
        };
        // Open and at rest, the content sits inside the shape clear of its rounded corners, so a
        // plain rectangle clip will do: no island-sized layer bitmap (about 2 MB with lyrics).
        // While anything moves or fades, clip to the real shape.
        let at_rest = matches!(self.view, ViewKey::Expanded(..))
            && alpha >= 0.999
            && self.slide.is_none()
            && self.leaving.is_none()
            && self.sw.done()
            && self.sh.done()
            && self.sr.done();
        if let Some(m) = morph {
            // the view being left: at its own size, fading out quickly as the shape changes
            let fade = 1.0 - smooth(self.view_age / MORPH_FADE);
            if fade > 0.01 {
                g.push_rr(shape, rad, fade);
                let oc = R::new(isl.cx() - m.old_w / 2.0, isl.y, m.old_w, m.old_h);
                let mut dummy = Ui { hits: Vec::new(), mouse: None };
                self.art_mode.set(2);
                match m.old {
                    ViewKey::Expanded(t, l) => self.draw_expanded(g, &mut dummy, oc, t, l),
                    ViewKey::Compact(live) => self.draw_compact(g, &mut dummy, oc, live),
                    _ => {}
                }
                g.pop();
            }
            self.art_mode.set(1);
            self.art_slot.set(None);
        }
        if at_rest {
            g.push_rr(shape, 0.0, 1.0);
        } else {
            g.push_rr(shape, rad, alpha.max(0.0));
        }
        // the closed pill's text glides with the hover instead of hopping row by row
        g.set_no_snap(matches!(self.view, ViewKey::Compact(_) | ViewKey::Line));
        match self.view {
            ViewKey::Compact(live) => self.draw_compact(g, ui, c, live),
            ViewKey::Line => self.draw_compact(g, ui, c, self.lives().0),
            ViewKey::Toast(_) => {
                if let Some(t) = &self.toast {
                    self.draw_toast(g, ui, c, t);
                }
            }
            ViewKey::Expanded(tab, lyr) => self.draw_expanded(g, ui, c, tab, lyr),
        }
        g.pop();
        g.set_no_snap(false);
        self.art_mode.set(0);
        if let (Some(m), Some((to, to_rad))) = (morph, self.art_slot.get()) {
            // the artwork, fully visible the whole way, from where it was to where it goes
            // a little ahead of the shape, so it arrives together with the text (which is laid
            // out at its final place from the start) instead of trailing behind it
            let p = 1.0 - (1.0 - self.morph_p(&m)).powi(2);
            let lerp = |a: f32, b: f32| a + (b - a) * p;
            let from = R::new(m.from.x + self.screen_cx, m.from.y, m.from.w, m.from.h);
            let r = R::new(lerp(from.x, to.x), lerp(from.y, to.y), lerp(from.w, to.w), lerp(from.h, to.h));
            let rr = lerp(m.from_rad, to_rad);
            g.push_rr(shape, rad, 1.0);
            self.draw_art_now(g, r, rr);
            g.pop();
            self.art_last.set(Some((R::new(r.x - self.screen_cx, r.y, r.w, r.h), rr)));
        }
    }

    // -------------------------------------------------------------- compact

    fn draw_art(&self, g: &Gfx, r: R, rad: f32) {
        match self.art_mode.get() {
            2 => return,
            1 => {
                self.art_slot.set(Some((r, rad)));
                return;
            }
            _ => {}
        }
        self.art_last.set(Some((R::new(r.x - self.screen_cx, r.y, r.w, r.h), rad)));
        self.draw_art_now(g, r, rad);
    }

    fn draw_art_now(&self, g: &Gfx, r: R, rad: f32) {
        match self.cur_art() {
            Some(b) => g.bitmap(b, r, rad),
            None => {
                g.rrect(r, rad, SURF_H);
                g.icon(ic::MUSIC, r, (r.h * 0.45).max(9.0), T2);
            }
        }
    }

    /// The pill's progress line: along the shape's very bottom edge, from one side to the other
    /// (hover growth included, clipped by the rounded corners), a pixel thicker while hovered.
    fn bottom_progress(&self, g: &Gfx, f: f32, col: C) {
        if f > 0.0 {
            let s = self.shape_r.get();
            let sc = self.gfx.scale;
            // gliding between two whole-pixel thicknesses, not jumping a pixel half way
            let px = |v: f32| (v * sc).round() / sc;
            let t = px(2.0) + (px(3.0) - px(2.0)) * self.hov.clamp(0.0, 1.0);
            g.rect(R::new(s.x, s.bottom() - t, s.w * f.clamp(0.0, 1.0), t), col);
        }
    }

    fn draw_eq(&self, g: &Gfx, x0: f32, cy: f32, playing: bool, acc: C) {
        let (bw, gap, maxh) = (3.0, 2.0, 13.0);
        let t = self.t0.elapsed().as_secs_f32();
        for i in 0..4 {
            let fi = i as f32;
            let h = if playing {
                let v = ((t * (5.0 + fi * 1.7) + fi * 1.3).sin() * 0.5 + 0.5) * 0.72 + 0.28;
                maxh * v
            } else {
                3.0
            };
            g.rrect(R::new(x0 + fi * (bw + gap), cy - h / 2.0, bw, h), 1.5, acc);
        }
    }

    /// Width of the secondary-activity segment at the right end of the closed island.
    fn segment_width(&self, sec: Option<Live>) -> f32 {
        let g = &self.gfx;
        match sec {
            Some(Live::Pomo) => 12.0 + 14.0 + 6.0 + g.measure("88:88", 12.5, 600) + 14.0,
            Some(Live::Task) => {
                let due = self.task_line().map(|t| t.1).unwrap_or_default();
                12.0 + 14.0 + 5.0 + g.measure(&due, 12.0, 600) + 14.0
            }
            _ => 0.0,
        }
    }

    fn draw_segment(&self, g: &Gfx, ui: &mut Ui, r: R, sec: Live) {
        // thin divider between the two activities
        g.line(r.x, r.cy() - 8.0, r.x, r.cy() + 8.0, 1.0, WHITE.a(0.16));
        match sec {
            Live::Pomo => {
                let col = self.pomo_color();
                let rem = self.pomo.remaining();
                let rcx = r.x + 12.0 + 7.0;
                g.circle_stroke(rcx, r.cy(), 6.5, col.a(0.25), 2.2);
                g.arc(rcx, r.cy(), 6.5, (rem / self.pomo.total) as f32, 2.2, col);
                g.text(&mmss(rem), R::new(rcx + 13.0, r.y, r.right() - rcx - 13.0, r.h), 12.5, 600, col, Al::L);
                ui.hit(r, Act::Tab(Tab::Focus));
            }
            Live::Task => {
                let Some((_, due, now_due)) = self.task_line() else { return };
                let col = if now_due { RED } else { feat(g, Tab::Tasks) };
                g.icon(ic::TASKS, R::new(r.x + 10.0, r.y, 18.0, r.h), 12.0, col);
                g.text(&due, R::new(r.x + 31.0, r.y, r.w - 31.0, r.h), 12.0, 600, col, Al::L);
                ui.hit(r, Act::Tab(Tab::Tasks));
            }
            _ => {}
        }
    }

    fn draw_compact(&self, g: &Gfx, ui: &mut Ui, full: R, live: Live) {
        // the secondary activity (if any) takes a segment at the right end; the primary gets the rest
        let sec = self.lives().1;
        let seg = self.segment_width(sec);
        if let Some(sec) = sec {
            self.draw_segment(g, ui, R::new(full.right() - seg, full.y, seg, full.h), sec);
        }
        let c = R::new(full.x, full.y, full.w - seg, full.h);
        match live {
            // idle: an empty notch, also while hovered (it only grows a little)
            Live::Idle => {}
            Live::Media | Live::Lyrics => {
                let Some(m) = &self.media else { return };
                let acc = self.accent();
                let s = c.h - 10.0;
                self.draw_art(g, R::new(c.x + 7.0, c.y + 5.0, s, s), 6.0);
                let eq_x = c.right() - 14.0 - 18.0;
                self.draw_eq(g, eq_x, c.cy(), m.playing, g.theme.spectrum.unwrap_or(acc));
                // between the artwork and the equalizer: the clock, or the lyric line. Switching,
                // the clock drifts up and fades while the line rises into place.
                let k = self.mini_k();
                let gap = 6.0 + 4.0 * k;
                let x0 = c.x + 7.0 + s + gap;
                let area = R::new(x0, c.y, eq_x - gap - x0, c.h);
                // one after the other, so the two never sit on top of each other: the clock is
                // gone by the middle, the line comes in during the second half
                let out = smooth(k / 0.45);
                let inn = smooth((k - 0.4) / 0.6);
                if out < 0.999 {
                    let r = R::new(area.x, area.y - 9.0 * out, area.w, area.h);
                    g.clock(&hm(now().secs), r, CLOCK_SIZE, 600, WHITE.a(1.0 - out), Al::C);
                }
                if inn > 0.001 {
                    self.draw_mini_line(g, R::new(area.x, area.y + 9.0 * (1.0 - inn), area.w, area.h), m, inn);
                }
                // the line hugs the pill's lower edge: not while a taller shape is still closing
                if m.dur > 0.0 && self.sh.x < PILL_H + 3.0 {
                    self.bottom_progress(g, (self.media_pos() / m.dur) as f32, g.theme.audio_bar.unwrap_or(acc).a(0.85));
                }
            }
            Live::Pomo => {
                let col = self.pomo_color();
                let rem = self.pomo.remaining();
                let label = if self.pomo.mode == PMode::Break { "Break" } else { "Focus" };
                let label = if self.pomo.paused { "Paused" } else { label };
                g.icon(ic::TIMER, R::new(c.x + 10.0, c.y, 20.0, c.h), 13.0, col);
                g.text(label, R::new(c.x + 34.0, c.y, 70.0, c.h), 12.5, 600, T2, Al::L);
                let txt = mmss(rem);
                let tr = R::new(c.right() - 70.0, c.y, 56.0, c.h);
                g.text(&txt, tr, 13.5, 600, col, Al::R);
                let rcx = tr.right() - g.measure(&txt, 13.5, 600) - 14.0;
                g.circle_stroke(rcx, c.cy(), 6.5, col.a(0.25), 2.2);
                g.arc(rcx, c.cy(), 6.5, (rem / self.pomo.total) as f32, 2.2, col);
                self.bottom_progress(g, 1.0 - (rem / self.pomo.total) as f32, col.a(0.85));
            }
            Live::Task => {
                let Some((text, due, now_due)) = self.task_line() else { return };
                let col = if now_due { RED } else { feat(g, Tab::Tasks) };
                g.icon(ic::TASKS, R::new(c.x + 10.0, c.y, 20.0, c.h), 13.0, col);
                let dw = g.measure(&due, 12.0, 600);
                g.text(&due, R::new(c.right() - 14.0 - dw, c.y, dw + 2.0, c.h), 12.0, 600, col, Al::R);
                g.text(&text, R::new(c.x + 36.0, c.y, c.w - 36.0 - dw - 24.0, c.h), 12.5, 500, WHITE.a(0.9), Al::L);
            }
        }
    }

    /// The current lyric line in the closed island. A new line slides up into place while the
    /// old one leaves upward (following the same scroll as the big lyrics view).
    /// `fade`: how far the line has come in (0..1) while the pill switches from the clock.
    fn draw_mini_line(&self, g: &Gfx, a: R, m: &MediaState, fade: f32) {
        let remember = |t: &str| {
            let mut last = self.mini_last_txt.borrow_mut();
            if last.as_str() != t {
                last.clear();
                last.push_str(t);
            }
        };
        let v = match &self.lyr {
            Lyr::Found(v) if self.lyr_key == m.key => v,
            _ => {
                // the lyrics are gone (next song): the last line fades out with the morph
                let last = self.mini_last_txt.borrow();
                if !last.is_empty() {
                    g.text(&last, a, MINI_SIZE, 600, WHITE.a(fade), Al::C);
                }
                return;
            }
        };
        let Some(cur) = self.cur_line() else {
            // before the first line: the song's title, dimmed
            g.text(&m.title, a, MINI_SIZE, 500, T2.a(T2.3 * fade), Al::C);
            remember(&m.title);
            return;
        };
        let line_text = |i: usize| {
            let t = v[i].text.as_str();
            if t.trim().is_empty() {
                "\u{266A}"
            } else {
                t
            }
        };
        g.push_rr(a, 0.0, 1.0);
        let sc = self.lyr_scroll;
        let first = sc.floor().max(0.0) as usize;
        for i in (first..v.len()).take(2) {
            let d = i as f32 - sc;
            if d.abs() >= 1.0 {
                continue;
            }
            let r = R::new(a.x, a.y + d * a.h * 0.8, a.w, a.h);
            g.text(line_text(i), r, MINI_SIZE, 600, WHITE.a((1.0 - d.abs()) * fade), Al::C);
        }
        g.pop();
        remember(line_text(cur));
    }

    // -------------------------------------------------------------- toast

    fn draw_ticon(&self, g: &Gfx, icon: &TIcon, r: R, acc: C) {
        match icon {
            TIcon::G(ch) => g.icon(*ch, r, 17.0, acc),
            TIcon::MicOff => draw_mic_off(g, r, 15.0, acc),
            TIcon::Txt(s) => g.text(s, r, 16.0, 700, acc, Al::C),
            TIcon::Battery(f, ch) => draw_battery(g, r.cx() - 12.0, r.cy(), 22.0, 12.0, *f, *ch),
            TIcon::Drop => {
                if let Some(geo) = g.drop_geo(R::new(r.cx() - 7.0, r.cy() - 9.5, 14.0, 19.0)) {
                    g.fill_geo(&geo, g.legible(g.text_alpha(acc)));
                }
            }
        }
    }

    fn draw_toast(&self, g: &Gfx, ui: &mut Ui, c: R, t: &Toast) {
        if let Some(lv) = t.level {
            ui.hit(c, Act::Toast);
            g.icon(
                if let TIcon::G(ch) = t.icon { ch } else { ic::VOL },
                R::new(c.x + 12.0, c.y, 26.0, c.h),
                15.0,
                WHITE,
            );
            let bar = R::new(c.x + 46.0, c.cy() - 3.0, c.w - 46.0 - 58.0, 6.0);
            g.rrect(bar, 3.0, WHITE.a(0.16));
            // the volume bar follows theme.conf `audio_bar` (brightness keeps its own color)
            let bar_c = if t.kind == K_VOL { g.theme.audio_bar.unwrap_or(t.accent) } else { t.accent };
            g.rrect(R::new(bar.x, bar.y, (bar.w * lv.clamp(0.0, 1.0)).max(6.0), bar.h), 3.0, bar_c);
            g.text(&format!("{}%", (lv * 100.0).round() as i32), R::new(c.right() - 54.0, c.y, 40.0, c.h), 13.0, 600, WHITE, Al::R);
            if t.kind == K_VOL {
                ui.hit(R::new(bar.x - 6.0, c.y, bar.w + 12.0, c.h), Act::VolBar(bar));
            }
            return;
        }
        ui.hit(c, Act::Toast);
        if t.sub.is_empty() {
            let ir = R::new(c.x + SHORT_PAD, c.y, SHORT_ICON, c.h);
            match &t.icon {
                TIcon::Txt(s) => g.text(s, ir, 15.0, 700, t.accent, Al::C),
                other => self.draw_ticon(g, other, ir, t.accent),
            }
            let tx = ir.right() + 8.0;
            g.text(&t.title, R::new(tx, c.y, c.right() - tx - SHORT_PAD + 4.0, c.h), SHORT_SIZE, 600, WHITE, Al::L);
            return;
        }
        let rad = TWO_ICON / 2.0;
        let cx = c.x + TWO_PAD + rad;
        g.circle(cx, c.cy(), rad, t.accent.a(0.18));
        let ir = R::new(cx - rad, c.cy() - rad, TWO_ICON, TWO_ICON);
        match &t.icon {
            TIcon::G(ch) => g.icon(*ch, ir, 14.0, t.accent),
            TIcon::MicOff => draw_mic_off(g, ir, 13.0, t.accent),
            TIcon::Txt(s) => g.text(s, ir, 12.5, 700, t.accent, Al::C),
            TIcon::Battery(f, ch) => draw_battery(g, ir.cx() - 10.0, ir.cy(), 18.0, 10.0, *f, *ch),
            TIcon::Drop => {
                if let Some(geo) = g.drop_geo(R::new(ir.cx() - 5.5, ir.cy() - 7.5, 11.0, 15.0)) {
                    g.fill_geo(&geo, g.legible(g.text_alpha(t.accent)));
                }
            }
        }
        let tx = ir.right() + TWO_GAP;
        let mut tw = c.right() - tx - TWO_PAD;
        if let Some(code) = self.qr_toast_code(t.id) {
            // right to left, so the main one (Open) ends up first; registered after the
            // notice itself, so they win over tapping it
            let mut x = c.right() - TWO_PAD;
            for (k, (label, a)) in qr_buttons(&code).iter().enumerate().rev() {
                let bw = qr_btn_w(g, label);
                let r = R::new(x - bw, c.cy() - 13.0, bw, 26.0);
                let hv = ui.hit(r, Act::ToastQr(*a));
                if k == 0 {
                    let ac = accent_btn(g, t.accent);
                    g.rrect(r, 13.0, if hv { ac.mix(WHITE, 0.2) } else { ac });
                    g.text(label, r, 11.5, 700, accent_fg(g), Al::C);
                } else {
                    g.rrect(r, 13.0, surf(g, hv));
                    g.text(label, r, 11.5, 600, btn_fg(g, WHITE), Al::C);
                }
                x -= bw + QR_BTN_GAP;
            }
            tw = x + QR_BTN_GAP - 12.0 - tx;
        }
        let mid = c.cy();
        g.text(&t.title, R::new(tx, mid - 17.0, tw, 18.0), TWO_TITLE, 600, WHITE, Al::L);
        g.text(&t.sub, R::new(tx, mid + 1.0, tw, 16.0), TWO_SUB, 400, T2, Al::L);
    }

    // -------------------------------------------------------------- expanded

    fn draw_expanded(&self, g: &Gfx, ui: &mut Ui, c: R, tab: Tab, lyr: bool) {
        let inner = R::new(c.x + 18.0, c.y + 12.0, c.w - 36.0, c.h - 26.0);
        // tab bar
        for (i, t) in TABS.iter().enumerate() {
            let b = R::new(inner.x + i as f32 * 36.0, inner.y, 32.0, 28.0);
            let hv = ui.hit(b, Act::Tab(*t));
            let active = *t == tab;
            if active {
                g.rrect(b, 10.0, surf_active(g));
            } else if hv {
                g.rrect(b, 10.0, surf(g, true));
            }
            // a feature with its own color (a ready-made look, or picked on the Colors page)
            // shows it on its tab icon even while that tab is not open
            let own = g.theme.feature[i];
            let col = match own {
                _ if active => tab_color(g, *t),
                Some(c) => c.a(if hv { 1.0 } else { 0.8 }),
                None if hv => WHITE,
                None => T3,
            };
            draw_tab_icon(g, *t, b, col);
        }
        // right: battery, date, clock
        let n = now();
        let mut x = inner.right();
        let clock = hm(n.secs);
        let cw = g.measure_clock(&clock, 13.5, 600);
        g.clock(&clock, R::new(x - cw - 2.0, inner.y, cw + 2.0, 28.0), 13.5, 600, WHITE, Al::R);
        x -= cw + 10.0;
        // without the year: room for the seventh tab
        let full_date = n.date_label();
        let date = full_date.rsplit_once(' ').map(|(d, _)| d).unwrap_or(&full_date);
        let dw = g.measure(date, 12.0, 500);
        g.text(date, R::new(x - dw - 2.0, inner.y, dw + 2.0, 28.0), 12.0, 500, T2, Al::R);
        x -= dw + 16.0;
        if let Some((ac, pct)) = self.power {
            g.text(&format!("{pct}%"), R::new(x - 36.0, inner.y, 36.0, 28.0), 12.0, 500, T3, Al::R);
            x -= 42.0;
            draw_battery(g, x - 24.0, inner.y + 14.0, 22.0, 11.0, pct as f32 / 100.0, ac);
        }
        match &self.slide {
            None => self.draw_body(g, ui, c, tab, lyr),
            Some(sl) => {
                // Slide between features: the old page leaves to one side while the new one
                // comes in from the other. Hits are suppressed until the slide finishes.
                let t = sl.t.clamp(0.0, 1.0);
                // eases out: moves at once, lands softly
                let p = 1.0 - (1.0 - t).powi(3);
                // pages sit side by side (carousel), so they never overlap; a page inside the
                // same tab only glides a little while the two cross-fade
                let dist = if sl.sub { 36.0 } else { (c.w + sl.from_w) / 2.0 + 24.0 };
                let (old_o, new_o) = if sl.sub {
                    // one after the other, barely overlapping, so the two pages never mix
                    let out = 1.0 - (t / 0.35).min(1.0);
                    let inn = ((t - 0.25) / 0.55).clamp(0.0, 1.0);
                    (out * out, inn * inn * (3.0 - 2.0 * inn))
                } else {
                    ((1.0 - p * 1.2).max(0.0), (0.25 + p).min(1.0))
                };
                let mut dummy = Ui { hits: Vec::new(), mouse: None };
                let old_c = R::new(c.cx() - sl.from_w / 2.0, c.y, sl.from_w, sl.from_h);
                let all = R::new(-1e4, -1e4, 2e4, 2e4);
                g.translate(-sl.dir * dist * p, 0.0);
                g.push_rr(all, 0.0, old_o);
                self.draw_body(g, &mut dummy, old_c, sl.from_tab, sl.from_lyr);
                g.pop();
                g.translate(sl.dir * dist * (1.0 - p), 0.0);
                g.push_rr(all, 0.0, new_o);
                self.draw_body(g, &mut dummy, c, tab, lyr);
                g.pop();
                g.translate(0.0, 0.0);
            }
        }
    }

    fn draw_body(&self, g: &Gfx, ui: &mut Ui, c: R, tab: Tab, lyr: bool) {
        let inner = R::new(c.x + 18.0, c.y + 12.0, c.w - 36.0, c.h - 26.0);
        let body = R::new(inner.x, inner.y + 42.0, inner.w, inner.h - 42.0);
        match tab {
            Tab::Media => self.draw_media(g, ui, body, lyr),
            Tab::Focus => self.draw_focus(g, ui, body),
            Tab::Water => self.draw_water(g, ui, body),
            Tab::Shelf => self.draw_shelf(g, ui, body),
            Tab::Tasks => self.draw_tasks(g, ui, body),
            Tab::Clipboard => self.draw_clipboard(g, ui, body),
            Tab::Settings if lyr => self.draw_colors(g, ui, body),
            Tab::Settings => self.draw_settings(g, ui, body),
        }
    }

    fn draw_media(&self, g: &Gfx, ui: &mut Ui, c: R, split: bool) {
        let Some(m) = &self.media else {
            g.icon(ic::MUSIC, R::new(c.x, c.y + 6.0, c.w, 40.0), 26.0, T3);
            g.text("Nothing playing", R::new(c.x, c.y + 50.0, c.w, 22.0), 15.0, 600, WHITE, Al::C);
            g.text(
                "Play something in Spotify, Apple Music or your browser",
                R::new(c.x, c.y + 72.0, c.w, 18.0),
                12.0,
                400,
                T3,
                Al::C,
            );
            return;
        };
        let acc = self.accent();
        let pos = self.media_pos();
        let left_w = if split { 320.0 } else { c.w };
        let (art, col) = if split {
            (R::new(c.x, c.y, 64.0, 64.0), R::new(c.x + 78.0, c.y + 4.0, left_w - 78.0, 60.0))
        } else {
            (R::new(c.x, c.y + 2.0, 100.0, 100.0), R::new(c.x + 116.0, c.y, c.w - 116.0, c.h))
        };
        self.draw_art(g, art, if split { 12.0 } else { 16.0 });
        if !split && !m.app.is_empty() {
            g.text(&m.app, R::new(art.x, art.bottom() + 4.0, art.w, 16.0), 11.0, 500, T3, Al::C);
        }
        // title / artist + lyrics toggle
        let lb = R::new(col.right() - 32.0, col.y - 2.0, 32.0, 28.0);
        let hv = ui.hit(lb, Act::Lyrics);
        if self.lyrics_open || hv {
            g.rrect(lb, 10.0, if self.lyrics_open { g.theme.button_active.unwrap_or(acc.a(0.22)) } else { surf(g, false) });
        }
        g.icon(ic::LYRICS, lb, 14.0, if self.lyrics_open { active_icon(g, acc) } else { T2 });
        // lyrics in the closed island: a small notch over two lines of text
        let mb = R::new(lb.x - 36.0, lb.y, 32.0, 28.0);
        let mini = self.data.settings.mini_lyrics;
        let hv = ui.hit(mb, Act::MiniLyrics);
        if mini || hv {
            g.rrect(mb, 10.0, if mini { g.theme.button_active.unwrap_or(acc.a(0.22)) } else { surf(g, false) });
        }
        let mc = g.ico(if mini { active_icon(g, acc) } else { T2 });
        g.rrect(R::new(mb.cx() - 7.0, mb.cy() - 6.5, 14.0, 5.0), 2.5, mc);
        g.line(mb.cx() - 6.0, mb.cy() + 2.0, mb.cx() + 6.0, mb.cy() + 2.0, 1.6, mc);
        g.line(mb.cx() - 4.0, mb.cy() + 5.5, mb.cx() + 4.0, mb.cy() + 5.5, 1.6, mc.a(0.7));
        let tw = col.w - 76.0;
        g.text(&m.title, R::new(col.x, col.y, tw, 22.0), 16.0, 600, WHITE, Al::L);
        let sub = if split && !m.app.is_empty() && !m.artist.is_empty() {
            format!("{} · {}", m.artist, m.app)
        } else if m.artist.is_empty() {
            m.app.clone()
        } else {
            m.artist.clone()
        };
        g.text(&sub, R::new(col.x, col.y + 22.0, tw, 18.0), 13.0, 400, T2, Al::L);

        // progress
        let (bx, bw, by) = if split { (c.x, left_w, c.y + 84.0) } else { (col.x, col.w, col.y + 56.0) };
        let bar = R::new(bx, by, bw, 4.0);
        let hit = R::new(bx, by - 8.0, bw, 20.0);
        let hv = m.can_seek && m.dur > 0.0 && ui.hit(hit, Act::Seek(bar));
        let bar = if hv { R::new(bx, by - 1.0, bw, 6.0) } else { bar };
        g.rrect(bar, bar.h / 2.0, WHITE.a(0.15));
        let f = if m.dur > 0.0 { (pos / m.dur) as f32 } else { 0.0 };
        if f > 0.0 {
            let bc = g.theme.audio_bar.unwrap_or(acc);
            g.rrect(R::new(bar.x, bar.y, (bar.w * f).max(bar.h), bar.h), bar.h / 2.0, if hv { bc } else { bc.a(0.9) });
        }
        if hv {
            // dot marking how far the song is
            let dx = bar.x + bar.w * f.clamp(0.0, 1.0);
            g.circle(dx, bar.cy() + 0.5, 7.5, BLACK.a(0.25));
            g.circle(dx, bar.cy(), 7.0, PURE_WHITE);
            // time under the cursor, in a small dark tooltip above the bar
            if let Some((mx, _)) = ui.mouse {
                let p = ((mx - bar.x) / bar.w).clamp(0.0, 1.0) as f64 * m.dur;
                let t = mmss(p);
                let w = g.measure(&t, 11.5, 600) + 16.0;
                let tx = (mx - w / 2.0).clamp(bx - 6.0, bx + bw - w + 6.0);
                let tip = R::new(tx, by - 30.0, w, 22.0);
                g.rrect(tip, 6.0, C(0.17, 0.17, 0.18, 0.96));
                g.text(&t, tip, 11.5, 600, PURE_WHITE, Al::C);
            }
        }
        g.text(&mmss(pos), R::new(bx, by + 8.0, 60.0, 14.0), 11.0, 500, T3, Al::L);
        if m.dur > 0.0 {
            g.text(&format!("-{}", mmss(m.dur - pos)), R::new(bx + bw - 60.0, by + 8.0, 60.0, 14.0), 11.0, 500, T3, Al::R);
        }

        // controls
        let cy = by + 40.0;
        let cx = bx + bw / 2.0;
        let pr = R::new(cx - 20.0, cy - 19.0, 40.0, 38.0);
        let hv = ui.hit(pr, Act::PlayPause);
        g.circle(pr.cx(), pr.cy(), 19.0, if hv { WHITE.a(0.22) } else { WHITE.a(0.12) });
        g.icon(if m.playing { ic::PAUSE } else { ic::PLAY }, pr, 17.0, WHITE);
        icon_btn(g, ui, R::new(cx - 72.0, cy - 17.0, 40.0, 34.0), ic::PREV, 15.0, WHITE, Act::Prev, false);
        icon_btn(g, ui, R::new(cx + 32.0, cy - 17.0, 40.0, 34.0), ic::NEXT, 15.0, WHITE, Act::Next, false);

        if split {
            let dx = c.x + left_w + 18.0;
            g.line(dx, c.y + 4.0, dx, c.bottom() - 4.0, 1.0, WHITE.a(0.1));
            let area = R::new(dx + 18.0, c.y - 6.0, c.right() - dx - 18.0, c.h + 10.0);
            self.draw_lyrics(g, ui, area, acc);
        }
    }

    fn draw_lyrics(&self, g: &Gfx, ui: &mut Ui, a: R, acc: C) {
        match &self.lyr {
            Lyr::Found(v) => {
                g.push_rr(a, 0.0, 1.0);
                let cur = self.cur_line();
                let lh = 27.0;
                let cy = a.cy();
                let s = self.lyr_scroll;
                let lo = (s.floor() as i64 - 4).max(0) as usize;
                let hi = ((s.ceil() as i64 + 5).max(0) as usize).min(v.len());
                for (i, l) in v.iter().enumerate().take(hi).skip(lo) {
                    let y = cy + (i as f32 - s) * lh - lh / 2.0;
                    if y + lh < a.y || y > a.bottom() {
                        continue;
                    }
                    let dist = (i as f32 - s).abs();
                    let is_cur = Some(i) == cur;
                    let lr = R::new(a.x, y, a.w, lh);
                    let hv = ui.hit(lr, Act::LyricLine(l.t));
                    let col = if is_cur {
                        WHITE
                    } else if hv {
                        g.theme.lyrics_hover.unwrap_or(acc).a(0.9)
                    } else {
                        WHITE.a((0.42 - dist * 0.07).max(0.12))
                    };
                    let (size, wt) = if is_cur { (16.0, 700) } else { (14.0, 500) };
                    g.text(&l.text, lr, size, wt, col, Al::L);
                }
                g.pop();
            }
            Lyr::Loading | Lyr::None => {
                g.text("Searching for synced lyrics…", a, 13.0, 500, T3, Al::C);
            }
            Lyr::Missing => {
                g.text("No synced lyrics found for this track", R::new(a.x, a.cy() - 18.0, a.w, 20.0), 13.0, 600, T2, Al::C);
                g.text("Lyrics come from lrclib.net", R::new(a.x, a.cy() + 2.0, a.w, 18.0), 11.5, 400, T3, Al::C);
            }
        }
    }

    fn draw_focus(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let p = &self.pomo;
        let col = self.pomo_color();
        let total = if p.active() { p.total } else { self.data.focus.focus_min as f64 * 60.0 };
        let rem = if p.active() { p.remaining() } else { total };
        let (rcx, rcy, rad) = (c.x + 58.0, c.y + 58.0, 52.0);
        g.circle_stroke(rcx, rcy, rad, WHITE.a(0.09), 7.0);
        g.arc(rcx, rcy, rad, (rem / total.max(1.0)) as f32, 7.0, col);
        g.text(&mmss(rem), R::new(rcx - 50.0, rcy - 17.0, 100.0, 30.0), 24.0, 600, WHITE, Al::C);
        let label = match (p.mode, p.paused) {
            (PMode::Idle, _) => "READY",
            (_, true) => "PAUSED",
            (PMode::Focus, _) => "FOCUS",
            (PMode::Break, _) => "BREAK",
        };
        g.text(label, R::new(rcx - 50.0, rcy + 12.0, 100.0, 14.0), 10.0, 700, if p.paused { T3 } else { col }, Al::C);

        let x = c.x + 140.0;
        let w = c.right() - x;
        let title = match p.mode {
            PMode::Idle => "Ready to focus",
            PMode::Focus => "Deep focus",
            PMode::Break => "Take a breather",
        };
        g.text(title, R::new(x, c.y, w, 22.0), 16.0, 600, WHITE, Al::L);
        let s = self.data.focus.sessions;
        g.text(
            &format!("{} session{} completed today", s, if s == 1 { "" } else { "s" }),
            R::new(x, c.y + 22.0, w, 18.0),
            12.0,
            400,
            T2,
            Al::L,
        );
        let by = c.y + 50.0;
        let btxt = match (p.mode, p.paused) {
            (PMode::Idle, _) => "Start focus",
            (_, true) => "Resume",
            _ => "Pause",
        };
        let b = R::new(x, by, 118.0, 34.0);
        let hv = ui.hit(b, Act::FocusToggle);
        let ac = accent_btn(g, col);
        g.rrect(b, 17.0, if hv { ac } else { ac.a(0.85) });
        g.text(btxt, b, 13.0, 600, accent_fg(g), Al::C);
        icon_btn(g, ui, R::new(x + 126.0, by, 34.0, 34.0), ic::RESET, 14.0, WHITE, Act::FocusReset, true);
        icon_btn(g, ui, R::new(x + 166.0, by, 34.0, 34.0), ic::NEXT, 14.0, WHITE, Act::FocusSkip, true);

        let sy = c.y + 98.0;
        let fw = stepper(g, ui, x, sy, "Focus", &format!("{} min", self.data.focus.focus_min), Act::FocusLen(false, -5), Act::FocusLen(false, 5), Field::FocusMin, self.edit_of(Field::FocusMin));
        stepper(g, ui, x + fw + 16.0, sy, "Break", &format!("{} min", self.data.focus.break_min), Act::FocusLen(true, -1), Act::FocusLen(true, 1), Field::BreakMin, self.edit_of(Field::BreakMin));
    }

    fn next_water_in(&self) -> Option<i64> {
        let w = &self.data.water;
        if w.total() >= w.goal_ml {
            return None;
        }
        let iv = w.interval_min as i64 * 60;
        let el = self.water_mark.elapsed().as_secs() as i64;
        Some(((iv - el).max(0) + 59) / 60)
    }

    fn draw_water(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let w = &self.data.water;
        let total = w.total();
        let f = (total as f32 / w.goal_ml.max(1) as f32).min(1.0);
        let dr = R::new(c.x + 6.0, c.y + 2.0, 74.0, 98.0);
        if let Some(geo) = g.drop_geo(dr) {
            g.fill_geo(&geo, WHITE.a(0.07));
            g.push_geo(&geo, 1.0);
            let fh = dr.h * f;
            g.rect(R::new(dr.x - 2.0, dr.bottom() - fh, dr.w + 4.0, fh + 2.0), feat(g, Tab::Water).a(0.9));
            g.pop();
            g.stroke_geo(&geo, feat(g, Tab::Water).a(0.5), 1.5);
        }
        g.text(&format!("{}%", (f * 100.0).round() as i32), R::new(dr.x, dr.y + 50.0, dr.w, 22.0), 14.0, 700, WHITE, Al::C);

        let x = c.x + 104.0;
        let cw = c.right() - x;
        let big = format!("{total} ml");
        g.text(&big, R::new(x, c.y - 2.0, cw, 30.0), 24.0, 600, WHITE, Al::L);
        let bw = g.measure(&big, 24.0, 600);
        g.text(&format!("of {} ml", w.goal_ml), R::new(x + bw + 8.0, c.y + 4.0, cw, 22.0), 13.0, 400, T2, Al::L);
        let bar = R::new(x, c.y + 34.0, cw, 6.0);
        g.rrect(bar, 3.0, WHITE.a(0.12));
        if f > 0.0 {
            g.rrect(R::new(bar.x, bar.y, (bar.w * f).max(6.0), 6.0), 3.0, feat(g, Tab::Water));
        }
        let note = match self.next_water_in() {
            None => "Daily goal reached — nice work!".to_string(),
            Some(_) if !w.remind => format!("Reminders off · {} drinks today", w.log.len()),
            Some(m) => format!("Next reminder in {m} min · {} drinks today", w.log.len()),
        };
        // reminders on/off: a small switch at the top right
        {
            let on = w.remind;
            let sr = R::new(c.right() - 112.0, c.y + 2.0, 112.0, 24.0);
            let hv = ui.hit(sr, Act::WaterRemind);
            if hv {
                g.rrect(sr, 12.0, surf(g, false));
            }
            g.text("Reminders", R::new(sr.x + 8.0, sr.y, 64.0, sr.h), 11.5, 500, btn_fg(g, if on { T2 } else { T3 }), Al::L);
            let track = R::new(sr.right() - 38.0, sr.cy() - 8.0, 30.0, 16.0);
            g.rrect(track, 8.0, if on { accent_btn(g, feat(g, Tab::Water)) } else { WHITE.a(0.22) });
            let kx = if on { track.right() - 8.0 } else { track.x + 8.0 };
            g.circle(kx, track.cy(), 6.0, if on { accent_fg(g) } else { WHITE.a(0.9) });
        }
        g.text(&note, R::new(x, c.y + 44.0, cw, 18.0), 11.5, 400, T3, Al::L);

        let by = c.y + 68.0;
        let b1 = R::new(x, by, 86.0, 32.0);
        let hv1 = ui.hover(b1);
        pill_btn(g, ui, b1, "+250 ml", btn_fg(g, WHITE), surf(g, hv1), Act::WaterAdd(250));
        let b2 = R::new(x + 94.0, by, 150.0, 32.0);
        pill_btn(g, ui, b2, &format!("+ Bottle · {} ml", w.bottle_ml), accent_fg(g), accent_btn(g, feat(g, Tab::Water)), Act::WaterAdd(w.bottle_ml));
        let mut cx2 = x + 252.0;
        if !w.log.is_empty() {
            icon_btn(g, ui, R::new(cx2, by, 34.0, 32.0), ic::UNDO, 14.0, WHITE, Act::WaterUndo, true);
            cx2 += 42.0;
        }
        // type any amount, Enter logs it
        let cr = R::new(cx2, by, (c.right() - cx2).min(80.0), 32.0);
        let ch = ui.hit(cr, Act::Edit(Field::DrinkMl));
        match self.edit_of(Field::DrinkMl) {
            Some((buf, caret)) => {
                g.rrect(cr, 16.0, WHITE.a(0.12));
                g.rrect_stroke(cr.inset(0.5, 0.5), 15.5, feat(g, Tab::Water).a(0.7), 1.2, false);
                let txt = format!("{buf} ml");
                let tw = g.measure(&txt, 12.5, 600);
                let tx = cr.cx() - tw / 2.0;
                g.text(&txt, R::new(tx, cr.y, tw + 4.0, cr.h), 12.5, 600, WHITE, Al::L);
                if caret {
                    let bw = g.measure(buf, 12.5, 600);
                    g.line(tx + bw + 1.0, cr.cy() - 7.0, tx + bw + 1.0, cr.cy() + 7.0, 1.3, feat(g, Tab::Water));
                }
            }
            None => {
                g.rrect(cr, 16.0, surf(g, ch));
                g.text("Custom", cr, 12.5, 600, btn_fg(g, T2), Al::C);
            }
        }

        let sy = c.y + 114.0;
        let mut sx = c.x;
        sx += stepper(g, ui, sx, sy, "Bottle", &format!("{} ml", w.bottle_ml), Act::WaterSet(0, -50), Act::WaterSet(0, 50), Field::Bottle, self.edit_of(Field::Bottle)) + 14.0;
        let goal = format!("{:.2} L", w.goal_ml as f32 / 1000.0);
        sx += stepper(g, ui, sx, sy, "Goal", goal.trim_end_matches('0'), Act::WaterSet(1, -250), Act::WaterSet(1, 250), Field::Goal, self.edit_of(Field::Goal)) + 14.0;
        stepper(g, ui, sx, sy, "Every", &format!("{} min", w.interval_min), Act::WaterSet(2, -15), Act::WaterSet(2, 15), Field::Interval, self.edit_of(Field::Interval));
    }

    fn draw_shelf(&self, g: &Gfx, ui: &mut Ui, c: R) {
        if let Some(p) = &self.qr_pop {
            if let Some(Some(codes)) = self.qr.get(p) {
                if !codes.is_empty() {
                    self.draw_qr_page(g, ui, c, p, codes);
                    return;
                }
            }
        }
        let files = &self.data.shelf;
        if files.is_empty() {
            let r = c.inset(2.0, 2.0);
            g.rrect(r, 16.0, if self.drag_hover { feat(g, Tab::Shelf).a(0.12) } else { WHITE.a(0.03) });
            g.rrect_stroke(r, 16.0, if self.drag_hover { feat(g, Tab::Shelf) } else { WHITE.a(0.22) }, 1.5, true);
            g.icon(ic::FOLDER, R::new(r.x, r.y + 18.0, r.w, 34.0), 24.0, if self.drag_hover { feat(g, Tab::Shelf) } else { T2 });
            g.text(
                if self.drag_hover { "Release to add to the shelf" } else { "Drop files here" },
                R::new(r.x, r.y + 56.0, r.w, 22.0),
                14.0,
                600,
                WHITE,
                Al::C,
            );
            g.text("Keep files handy, then drag them out into chats, emails or folders", R::new(r.x, r.y + 78.0, r.w, 18.0), 11.5, 400, T3, Al::C);
            return;
        }
        let (tw, th, gap) = (SHELF_TW, SHELF_TH, SHELF_GAP);
        let per = shelf_per(c.w);
        // tiles slide sideways when scrolling: draw every tile that is at least partly
        // visible, clipped to the shelf row; only fully visible tiles take clicks
        let pos = self.shelf_x.clamp(0.0, files.len().saturating_sub(per) as f32);
        let first = pos.floor() as usize;
        let row = R::new(c.x, c.y - 2.0, (per as f32) * (tw + gap) - gap, th + 4.0);
        g.push_rr(row, 14.0, 1.0);
        for (k, path) in files.iter().enumerate().skip(first).take(per + 1) {
            let off = k as f32 - pos;
            let t = R::new(c.x + off * (tw + gap), c.y, tw, th);
            let whole = off > -0.02 && off < per as f32 - 0.98;
            // fade tiles as they slide out of the row
            let fade = if whole { 1.0 } else { (1.0 - (off.min(0.0).abs()).max((off - (per as f32 - 1.0)).max(0.0))).clamp(0.0, 1.0) };
            if fade <= 0.01 {
                continue;
            }
            let layered = fade < 0.999;
            if layered {
                g.push_rr(R::new(-1e4, -1e4, 2e4, 2e4), 0.0, fade);
            }
            let hv = if whole { ui.hit(t, Act::ShelfItem(k)) } else { false };
            let sel = self.shelf_sel.contains(path);
            if sel {
                g.rrect(t, 14.0, if hv { feat(g, Tab::Shelf).a(0.28) } else { feat(g, Tab::Shelf).a(0.16) });
                g.rrect_stroke(t.inset(0.75, 0.75), 13.5, feat(g, Tab::Shelf).a(0.85), 1.5, false);
            } else {
                g.rrect(t, 14.0, surf(g, hv));
            }
            let name = std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or(path.clone());
            let is_dir = std::path::Path::new(path).is_dir();
            let ext = std::path::Path::new(path)
                .extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            let badge = R::new(t.cx() - 17.0, t.y + 10.0, 34.0, 40.0);
            if let Some(Some(b)) = self.thumbs.get(path) {
                let pr = R::new(t.x + 10.0, t.y + 8.0, t.w - 20.0, 45.0);
                g.bitmap(b, pr, 7.0);
                g.rrect_stroke(pr.inset(0.5, 0.5), 6.5, WHITE.a(0.14), 1.0, false);
                // the file type, small, in the corner
                let e: String = ext.chars().take(4).collect();
                if !e.is_empty() {
                    let lw = g.measure(&e, 8.5, 700) + 8.0;
                    let lr = R::new(pr.right() - lw - 3.0, pr.bottom() - 15.0, lw, 12.0);
                    let bc = ext_color(g, &ext);
                    g.rrect(lr, 4.0, bc.a(0.92));
                    g.text(&e, lr, 8.5, 700, on_color(bc), Al::C);
                }
                // a QR code in the picture: the badge shows what it says
                if let Some(Some(codes)) = self.qr.get(path) {
                    if !codes.is_empty() {
                        // small, so the file type still fits beside it: the QR sign, and how
                        // many codes when there are more
                        let count = if codes.len() > 1 { codes.len().min(9).to_string() } else { String::new() };
                        let lw = if count.is_empty() { 18.0 } else { 15.0 + g.measure(&count, 9.0, 700) + 4.0 };
                        let qr_r = R::new(pr.x + 3.0, pr.bottom() - 19.0, lw, 16.0);
                        let qh = whole && ui.hit(qr_r, Act::QrBadge(k));
                        let bc = feat(g, Tab::Shelf);
                        let bg = if qh { bc.mix(WHITE, 0.25) } else { bc };
                        g.rrect(qr_r, 5.0, bg);
                        g.icon(ic::QR, R::new(qr_r.x + 1.0, qr_r.y, 16.0, qr_r.h), 10.0, on_color(bg));
                        if !count.is_empty() {
                            g.text(&count, R::new(qr_r.x + 15.0, qr_r.y, lw - 15.0, qr_r.h), 9.0, 700, on_color(bg), Al::L);
                        }
                    }
                }
            } else if is_dir {
                g.icon(ic::FOLDER, badge, 28.0, feat(g, Tab::Shelf));
            } else {
                let bc = ext_color(g, &ext);
                let fg = on_color(bc);
                g.rrect(badge, 7.0, bc.a(0.9));
                g.rrect(R::new(badge.right() - 10.0, badge.y, 10.0, 10.0), 3.0, PURE_WHITE.a(0.35));
                let e: String = ext.chars().take(4).collect();
                if e.is_empty() {
                    g.icon(ic::DOC, badge, 16.0, fg);
                } else {
                    g.text(&e, R::new(badge.x, badge.y + 14.0, badge.w, 20.0), 10.0, 700, fg, Al::C);
                }
            }
            g.text_wrap(&name, R::new(t.x + 6.0, t.y + 56.0, t.w - 12.0, 32.0), 11.0, 500, WHITE.a(0.85), Al::C);
            // selection circle: always shown in selection mode, otherwise on hover
            if hv || sel || !self.shelf_sel.is_empty() {
                let cr = R::new(t.x + 4.0, t.y + 4.0, 18.0, 18.0);
                let ch = ui.hit(cr, Act::ShelfCheck(k));
                if sel {
                    g.circle(cr.cx(), cr.cy(), 9.0, feat(g, Tab::Shelf));
                    g.icon(ic::CHECK, cr, 9.0, BLACK);
                } else {
                    g.circle(cr.cx(), cr.cy(), 9.0, BLACK.a(0.6));
                    g.circle_stroke(cr.cx(), cr.cy(), 8.0, if ch { WHITE } else { WHITE.a(0.55) }, 1.3);
                }
            }
            if hv {
                let xr = R::new(t.right() - 22.0, t.y + 4.0, 18.0, 18.0);
                let xh = ui.hit(xr, Act::ShelfRemove(k));
                g.circle(xr.cx(), xr.cy(), 9.0, if xh { RED } else { BLACK.a(0.7) });
                g.icon(ic::CLOSE, xr, 8.0, WHITE);
            }
            if layered {
                g.pop();
            }
        }
        g.pop();
        // scroll position: a thin bar under the row when not everything fits
        if files.len() > per {
            let track = R::new(row.x, c.y + th + 3.0, row.w, 2.0);
            g.rrect(track, 1.0, WHITE.a(0.12));
            let span = files.len() as f32;
            let tw2 = track.w * per as f32 / span;
            let tx = track.x + (track.w - tw2) * (pos / (span - per as f32)).clamp(0.0, 1.0);
            g.rrect(R::new(tx, track.y, tw2, 2.0), 1.0, WHITE.a(0.55));
        }
        let fy = c.y + th + 8.0;
        let nsel = files.iter().filter(|f| self.shelf_sel.contains(f)).count();
        let info = if nsel > 0 {
            format!("{nsel} selected · drag a selected file to take them all")
        } else if files.len() > per {
            format!("{} items · scroll for more · Ctrl+click to select", files.len())
        } else {
            format!("{} item{} · click to open · Ctrl+click to select", files.len(), if files.len() == 1 { "" } else { "s" })
        };
        g.text(&info, R::new(c.x, fy, c.w - 200.0, 20.0), 11.5, 400, if nsel > 0 { feat(g, Tab::Shelf).mix(WHITE, 0.4) } else { T3 }, Al::L);
        // press and drag this to carry every file on the shelf at once
        let dr = R::new(c.right() - 74.0 - 8.0 - 100.0, fy - 2.0, 100.0, 24.0);
        let hv = ui.hit(dr, Act::ShelfAll);
        if nsel > 0 {
            let ac = accent_btn(g, feat(g, Tab::Shelf));
            g.rrect(dr, 12.0, if hv { ac.mix(WHITE, 0.2) } else { ac });
            g.text("Deselect", dr, 11.5, 700, accent_fg(g), Al::C);
        } else {
            g.rrect(dr, 12.0, if hv { feat(g, Tab::Shelf).a(0.35) } else { feat(g, Tab::Shelf).a(0.18) });
            g.text(&format!("Drag all ({})", files.len()), dr, 11.5, 600, feat(g, Tab::Shelf).mix(WHITE, 0.35), Al::C);
        }
        let cr = R::new(c.right() - 74.0, fy - 2.0, 74.0, 24.0);
        let hv = ui.hit(cr, Act::ShelfClear);
        g.rrect(cr, 12.0, if hv { RED.a(0.25) } else { surf(g, false) });
        g.text("Clear all", cr, 11.5, 600, if hv { RED } else { btn_fg(g, T2) }, Al::C);
        if self.drag_hover {
            g.rrect_stroke(c.inset(-4.0, -4.0), 16.0, feat(g, Tab::Shelf), 1.5, true);
        }
    }

    /// What the QR codes in a Shelf image say, in place of the tiles (from its QR badge or the
    /// QR notice): one row each, with Open / Copy.
    fn draw_qr_page(&self, g: &Gfx, ui: &mut Ui, c: R, path: &str, codes: &[String]) {
        let acc = feat(g, Tab::Shelf);
        // header: back to the Shelf, which picture
        let br = R::new(c.x, c.y - 2.0, 26.0, 24.0);
        let bh = ui.hit(br, Act::QrClose);
        g.rrect(br, 8.0, surf(g, bh));
        g.icon(ic::LEFT, br, 10.0, if bh { WHITE } else { T2 });
        let name = std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let what = if codes.len() == 1 { "QR code".to_string() } else { format!("{} QR codes", codes.len()) };
        let hx = br.right() + 10.0;
        let ww = g.measure(&what, 13.0, 600);
        g.text(&what, R::new(hx, c.y - 2.0, ww + 2.0, 24.0), 13.0, 600, WHITE, Al::L);
        g.text(&format!("in {name}"), R::new(hx + ww + 6.0, c.y - 2.0, c.right() - hx - ww - 6.0, 24.0), 11.5, 400, T3, Al::L);
        // one row per code
        let ly = c.y + 28.0;
        let rh = 33.0;
        let rows = ((c.bottom() - ly + 3.0) / rh).floor().max(1.0) as usize;
        let copied = self.qr_copied.filter(|(t, _)| t.elapsed() < Duration::from_millis(1400)).map(|(_, i)| i);
        for (i, code) in codes.iter().enumerate().take(rows) {
            let r = R::new(c.x, ly + i as f32 * rh, c.w, rh - 3.0);
            let hv = ui.hover(r);
            g.rrect(r, 9.0, if hv { WHITE.a(0.08) } else { WHITE.a(0.04) });
            let kind = qr::kind(code);
            let icon = match kind {
                qr::Kind::Link(_) => ic::LINK,
                qr::Kind::Wifi(..) => ic::WIFI,
                qr::Kind::Text(_) => ic::DOC,
            };
            g.icon(icon, R::new(r.x + 8.0, r.y, 18.0, r.h), 12.0, acc);
            // buttons from the right
            let mut x = r.right() - 4.0;
            for (k, (label, a)) in qr_buttons(code).iter().enumerate().rev() {
                let done = *a == 1 && copied == Some(i);
                let label = if done { "Copied" } else { *label };
                let bw = qr_btn_w(g, label);
                let b = R::new(x - bw, r.y + 3.0, bw, r.h - 6.0);
                let bh = ui.hit(b, Act::QrDo(i, *a));
                if k == 0 && !done {
                    let ac = accent_btn(g, acc);
                    g.rrect(b, 12.0, if bh { ac.mix(WHITE, 0.2) } else { ac });
                    g.text(label, b, 11.5, 700, accent_fg(g), Al::C);
                } else {
                    g.rrect(b, 12.0, if done { acc.a(0.25) } else { surf(g, bh) });
                    g.text(label, b, 11.5, 600, if done { acc.mix(WHITE, 0.35) } else { btn_fg(g, WHITE) }, Al::C);
                }
                x -= bw + QR_BTN_GAP;
            }
            let (_, line) = qr_label(code);
            let line = match &kind {
                qr::Kind::Wifi(..) => format!("Wi-Fi · {line}"),
                _ => line,
            };
            let tr = R::new(r.x + 32.0, r.y, x - 6.0 - (r.x + 32.0), r.h);
            // keyed by row, with its own high bit (tasks and clipboard rows use others)
            self.marquee_text(g, (1 << 61) | i as u64, hv, &line, tr, 12.5, 500, WHITE);
        }
        if codes.len() > rows {
            let more = format!("+{} more", codes.len() - rows);
            g.text(&more, R::new(c.right() - 120.0, c.y - 2.0, 120.0, 24.0), 11.5, 500, T3, Al::R);
        }
    }

    /// Ask for previews of the image tiles in view (and the next page, so scrolling finds
    /// them ready); forget the ones no longer on the Shelf.
    fn sync_thumbs(&mut self) {
        if !self.expanded || self.tab != Tab::Shelf {
            return;
        }
        let shelf = &self.data.shelf;
        if self.thumbs.len() > shelf.len() || self.thumbs.keys().any(|k| !shelf.contains(k)) {
            self.thumbs.retain(|k, _| shelf.contains(k));
        }
        let per = shelf_per(SHELF_BODY_W);
        let first = (self.shelf_x.max(0.0).floor() as usize).saturating_sub(per);
        for p in shelf.iter().skip(first).take(per * 3 + 1) {
            if thumbs::is_image(p) && !self.thumbs.contains_key(p) {
                self.thumbs.insert(p.clone(), None);
                thumbs::request(p);
            }
            if thumbs::is_image(p) && !self.qr.contains_key(p) {
                self.qr.insert(p.clone(), None);
                qr::request(p, false);
            }
        }
        if self.qr.keys().any(|k| !shelf.contains(k)) {
            self.qr.retain(|k, _| shelf.contains(k));
        }
    }

    /// (text, caret visible) when `f` is being edited.
    fn edit_of(&self, f: Field) -> Option<(&str, bool)> {
        let e = self.edit.as_ref().filter(|e| e.field == f)?;
        Some((e.buf.as_str(), (self.t0.elapsed().as_millis() / 530) % 2 == 0))
    }

    fn start_edit(&mut self, f: Field) {
        if self.edit.as_ref().map(|e| e.field) == Some(f) {
            return;
        }
        self.commit_edit();
        let w = &self.data.water;
        let buf = match f {
            Field::FocusMin => self.data.focus.focus_min.to_string(),
            Field::BreakMin => self.data.focus.break_min.to_string(),
            Field::Bottle => w.bottle_ml.to_string(),
            Field::Goal => w.goal_ml.to_string(),
            Field::Interval => w.interval_min.to_string(),
            Field::DrinkMl => String::new(),
            Field::Hex => theme::hex(self.col_color()),
            Field::TaskTime => self.draft_due().map(hm).unwrap_or_default(),
        };
        self.edit = Some(Edit { field: f, buf, fresh: true });
        self.input_focus = false;
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.hwnd);
        }
    }

    /// Apply the typed value. Invalid input leaves the old value untouched.
    fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let t = e.buf.trim().to_string();
        match e.field {
            Field::Hex => {
                let h = t.trim_start_matches('#');
                // #ABC is short for #AABBCC
                let full = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_string() };
                if full.len() == 6 {
                    if let Some(c) = theme::color(&full) {
                        self.col_hsv = theme::to_hsv(c);
                        self.col_apply(c, true);
                    }
                }
            }
            Field::TaskTime => {
                if let Some(v) = parse_due_on(&t, now().secs, self.sel_day) {
                    self.task_due = v;
                    self.follow_due();
                }
            }
            f => {
                let Some(v) = parse_amount(&t, f == Field::Goal || f == Field::Bottle) else { return };
                let w = &mut self.data.water;
                match f {
                    Field::FocusMin => self.data.focus.focus_min = v.clamp(1, 180),
                    Field::BreakMin => self.data.focus.break_min = v.clamp(1, 60),
                    Field::Bottle => w.bottle_ml = v.clamp(50, 3000),
                    Field::Goal => w.goal_ml = v.clamp(250, 10000),
                    Field::Interval => w.interval_min = v.clamp(5, 480),
                    Field::DrinkMl => {
                        if v > 0 {
                            let before = w.total();
                            w.log.push(v.min(3000));
                            if before < w.goal_ml && w.total() >= w.goal_ml {
                                sound::chime();
                            }
                            self.water_mark = Instant::now();
                        }
                    }
                    Field::TaskTime | Field::Hex => {}
                }
                self.dirty = true;
            }
        }
        self.kick();
    }

    /// Furthest the shelf can scroll (first visible tile index).
    fn shelf_max(&self) -> usize {
        self.data.shelf.len().saturating_sub(shelf_per(SHELF_BODY_W))
    }

    fn toggle_sel(&mut self, i: usize) {
        if let Some(p) = self.data.shelf.get(i).cloned() {
            if let Some(pos) = self.shelf_sel.iter().position(|x| *x == p) {
                self.shelf_sel.remove(pos);
            } else {
                self.shelf_sel.push(p);
            }
        }
    }

    fn selected_paths(&self) -> Vec<String> {
        self.data.shelf.iter().filter(|p| self.shelf_sel.contains(p)).cloned().collect()
    }

    fn draw_clipboard(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let n = now().secs;
        let drop = self.drag_hover && self.drag_text;
        if self.clips.is_empty() {
            if drop {
                let r = c.inset(2.0, 2.0);
                g.rrect(r, 16.0, feat(g, Tab::Clipboard).a(0.12));
                g.rrect_stroke(r, 16.0, feat(g, Tab::Clipboard), 1.5, true);
            }
            // icon 34 + gap 6 + title 22 + subtitle 18, centered in the body
            let y = c.y + ((c.h - 80.0) / 2.0).max(0.0);
            g.icon(ic::CLIP, R::new(c.x, y, c.w, 34.0), 24.0, if drop { feat(g, Tab::Clipboard) } else { T3 });
            let title = if drop { "Release to save the text" } else { "Nothing copied yet" };
            g.text(title, R::new(c.x, y + 40.0, c.w, 22.0), 14.0, 600, if drop { WHITE } else { T2 }, Al::C);
            g.text("Text you copy or drop on Veyra shows up here. Click an entry to copy it again.", R::new(c.x, y + 62.0, c.w, 18.0), 11.5, 400, T3, Al::C);
            return;
        }
        // header: count + Clear all
        let count = self.clips.len();
        let cr = R::new(c.right() - 74.0, c.y, 74.0, 24.0);
        let ar = R::new(cr.x - 8.0 - 84.0, c.y, 84.0, 24.0);
        // the long header when it fits beside the buttons, the short one otherwise
        let what = if count == 1 { "entry" } else { "entries" };
        let room = ar.x - c.x - 10.0;
        let long = format!("{count} {what} · click to copy again · kept until Veyra closes");
        let head = if g.measure(&long, 11.5, 400) <= room { long } else { format!("{count} {what} · click to copy again") };
        g.text(&head, R::new(c.x + 2.0, c.y, room, 24.0), 11.5, 400, T3, Al::L);
        let done = self.clip_all_at.map(|t| t.elapsed() < Duration::from_millis(1400)).unwrap_or(false);
        let hv = ui.hit(ar, Act::ClipCopyAll);
        g.rrect(ar, 12.0, if hv || done { feat(g, Tab::Clipboard).a(0.3) } else { feat(g, Tab::Clipboard).a(0.16) });
        if done {
            let tw = g.measure("Copied", 11.5, 600);
            let x = ar.cx() - (tw + 14.0) / 2.0;
            g.icon(ic::CHECK, R::new(x, ar.y, 10.0, ar.h), 10.0, feat(g, Tab::Clipboard).mix(WHITE, 0.35));
            g.text("Copied", R::new(x + 14.0, ar.y, tw + 2.0, ar.h), 11.5, 600, feat(g, Tab::Clipboard).mix(WHITE, 0.35), Al::L);
        } else {
            g.text("Copy all", ar, 11.5, 600, feat(g, Tab::Clipboard).mix(WHITE, 0.35), Al::C);
        }
        let hv = ui.hit(cr, Act::ClipClear);
        g.rrect(cr, 12.0, if hv { RED.a(0.25) } else { surf(g, false) });
        g.text("Clear all", cr, 11.5, 600, if hv { RED } else { btn_fg(g, T2) }, Al::C);

        // list
        let ly = c.y + 32.0;
        let rh = 30.0;
        let rows = ((c.bottom() - ly) / rh).floor().max(1.0) as usize;
        let start = self.clip_scroll.min(count.saturating_sub(rows));
        let copied = self.clip_copied.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_millis(1400)).map(|(_, s)| s.as_str());
        let dropped = self.clip_dropped.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_millis(1400)).map(|(_, s)| s.as_str());
        for (k, item) in self.clips.iter().enumerate().skip(start).take(rows) {
            let r = R::new(c.x, ly + (k - start) as f32 * rh, c.w, rh - 2.0);
            // the whole row copies; the x on top of it removes (registered last, so it wins)
            let hv = ui.hit(r, Act::ClipCopy(k));
            g.rrect(r, 9.0, if hv { WHITE.a(0.08) } else { WHITE.a(0.03) });
            let right = 64.0;
            // keyed by position, with a high bit so it never matches a task id
            self.marquee_text(g, (1 << 62) | k as u64, hv, &clip::preview(&item.text), R::new(r.x + 12.0, r.y, r.w - 12.0 - right, r.h), 13.0, 400, WHITE);
            let mark = if copied == Some(item.text.as_str()) {
                Some("Copied")
            } else if dropped == Some(item.text.as_str()) {
                Some("Saved")
            } else {
                None
            };
            if let Some(m) = mark {
                g.icon(ic::CHECK, R::new(r.right() - 62.0, r.y, 16.0, r.h), 10.0, feat(g, Tab::Clipboard));
                g.text(m, R::new(r.right() - 46.0, r.y, 42.0, r.h), 11.5, 600, feat(g, Tab::Clipboard), Al::L);
            } else if hv {
                let xr = R::new(r.right() - 30.0, r.y + 2.0, 26.0, r.h - 4.0);
                let xh = ui.hit(xr, Act::ClipDel(k));
                if xh {
                    g.rrect(xr, 8.0, RED.a(0.2));
                }
                g.icon(ic::CLOSE, xr, 9.0, if xh { RED } else { T2 });
            } else {
                g.text(&ago(n - item.at), R::new(r.right() - right, r.y, right - 10.0, r.h), 11.5, 500, T3, Al::R);
            }
        }
        // a hint that there is more below / above
        if count > rows {
            let shown = format!("{}-{} of {count}", start + 1, (start + rows).min(count));
            g.text(&shown, R::new(c.x, c.bottom() - 2.0, c.w - 4.0, 14.0), 10.5, 500, T3, Al::R);
        }
        if drop {
            g.rrect_stroke(c.inset(-4.0, -4.0), 16.0, feat(g, Tab::Clipboard), 1.5, true);
        }
    }

    /// One line of text in `r`, cut with "…" as usual. Under the mouse, a line too long to fit
    /// slides left until its end shows, waits, and slides back. Returns how far it is slid.
    #[allow(clippy::too_many_arguments)]
    fn marquee_text(&self, g: &Gfx, key: u64, hover: bool, s: &str, r: R, size: f32, weight: u32, c: C) -> f32 {
        let tw = g.measure(s, size, weight);
        let over = tw - r.w;
        if !hover || over <= 1.0 {
            g.text(s, r, size, weight, c, Al::L);
            return 0.0;
        }
        let now = Instant::now();
        let start = match self.marquee_at.get() {
            Some((k, t)) if k == key => t,
            _ => {
                self.marquee_at.set(Some((key, now)));
                now
            }
        };
        self.marquee_on.set(true);
        // a short wait first (just passing by doesn't start it), ~45 px a second, a pause at the end
        const WAIT: f32 = 0.6;
        const HOLD: f32 = 1.4;
        let dist = over + 8.0;
        let go = (dist / 45.0).max(0.5);
        let t = now.duration_since(start).as_secs_f32() % (WAIT + go + HOLD + go);
        let f = if t < WAIT {
            0.0
        } else if t < WAIT + go {
            smooth((t - WAIT) / go)
        } else if t < WAIT + go + HOLD {
            1.0
        } else {
            1.0 - smooth((t - WAIT - go - HOLD) / go)
        };
        if f <= 0.0 {
            g.text(s, r, size, weight, c, Al::L);
            return 0.0;
        }
        let off = dist * f;
        g.push_rr(r, 0.0, 1.0);
        g.text(s, R::new(r.x - off, r.y, tw + 20.0, r.h), size, weight, c, Al::L);
        g.pop();
        off
    }

    /// Tasks shown for the picked day: the ones due that day; on today also the ones without
    /// a time and the unfinished ones from earlier days.
    fn day_tasks(&self) -> Vec<&Task> {
        let today = now().day();
        let sel = self.sel_day;
        let mut v: Vec<&Task> = self
            .data
            .tasks
            .iter()
            .filter(|t| match t.due {
                Some(d) => {
                    let dd = d.div_euclid(86400);
                    dd == sel || (sel == today && dd < today && !t.done)
                }
                None => sel == today,
            })
            .collect();
        v.sort_by_key(|t| (t.done, t.due.is_none(), t.due.unwrap_or(0), t.id));
        v
    }

    fn draw_calendar(&self, g: &Gfx, ui: &mut Ui, r: R, today: i64, now_s: i64) {
        let (y, m) = self.cal_month;
        // header: month (click: back to today) and the arrows
        let title = format!("{} {}", MONTHS[(m - 1).clamp(0, 11) as usize], y);
        let tw = g.measure(&title, 12.5, 600);
        let tr = R::new(r.x, r.y, tw + 10.0, 22.0);
        let th = ui.hit(tr, Act::CalToday);
        g.text(&title, R::new(r.x + 4.0, r.y, tw + 4.0, 22.0), 12.5, 600, if th { feat(g, Tab::Tasks) } else { WHITE }, Al::L);
        let nr = R::new(r.right() - 22.0, r.y, 22.0, 22.0);
        let pr = R::new(nr.x - 24.0, r.y, 22.0, 22.0);
        icon_btn(g, ui, pr, ic::LEFT, 9.0, T2, Act::CalMonth(-1), false);
        icon_btn(g, ui, nr, ic::RIGHT, 9.0, T2, Act::CalMonth(1), false);
        // weekdays, Monday first
        let cw = r.w / 7.0;
        let wy = r.y + 25.0;
        for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
            g.text(d, R::new(r.x + i as f32 * cw, wy, cw, 12.0), 9.5, 600, T3, Al::C);
        }
        // which days have tasks: (unfinished, overdue)
        let mut marks: HashMap<i64, (bool, bool)> = HashMap::new();
        for t in &self.data.tasks {
            if let Some(d) = t.due {
                let e = marks.entry(d.div_euclid(86400)).or_insert((false, false));
                if !t.done {
                    e.0 = true;
                    e.1 |= d <= now_s;
                }
            }
        }
        // the days: always 6 full rows (42 days), the gaps filled with the days before and
        // after, so every month looks the same and the grid never jumps
        let first = days_from_civil(y, m, 1);
        let start = first - weekday(first);
        let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        let next = days_from_civil(ny, nm, 1);
        let gy = wy + 14.0;
        let rh = ((r.bottom() - gy) / 6.0).min(30.0);
        let rad = (rh / 2.0 - 1.0).min(cw / 2.0 - 1.5);
        for k in 0..42 {
            let day = start + k;
            let cell = R::new(r.x + (k % 7) as f32 * cw, gy + (k / 7) as f32 * rh, cw, rh);
            let (cx, cy) = (cell.cx(), cell.cy());
            let hv = ui.hit(cell, Act::CalDay(day));
            let sel = day == self.sel_day;
            let in_month = day >= first && day < next;
            if sel {
                g.circle(cx, cy, rad, accent_btn(g, feat(g, Tab::Tasks)));
            } else if hv {
                g.circle(cx, cy, rad, surf(g, true));
            }
            if day == today && !sel {
                g.circle_stroke(cx, cy, rad - 0.6, feat(g, Tab::Tasks).a(0.85), 1.2);
            }
            let fg = if sel {
                accent_fg(g)
            } else if day == today {
                feat(g, Tab::Tasks)
            } else if !in_month {
                WHITE.a(0.22)
            } else if day < today {
                T2
            } else {
                WHITE
            };
            let (_, _, dn) = civil_from_days(day);
            let bold = if sel || day == today { 700 } else { 500 };
            let nr = R::new(cell.x, cy - 9.0, cell.w, 14.0);
            if !in_month && !sel && day != today {
                // neighbouring months stay clearly fainter (not lifted by text_contrast)
                g.text_dim(&dn.to_string(), nr, 11.0, 500, fg, Al::C);
            } else {
                g.text(&dn.to_string(), nr, 11.0, bold, fg, Al::C);
            }
            if let Some(&(open, over)) = marks.get(&day) {
                let col = if sel {
                    accent_fg(g)
                } else if over {
                    RED
                } else if open {
                    feat(g, Tab::Tasks)
                } else {
                    T3
                };
                g.circle(cx, cy + rad - 3.6, 1.6, if in_month || sel { col } else { col.a(col.3 * 0.5) });
            }
        }
    }

    fn draw_tasks(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let n = now();
        let today = n.day();
        // calendar on the left, the picked day on the right
        let cal = R::new(c.x, c.y, 196.0, c.h);
        self.draw_calendar(g, ui, cal, today, n.secs);
        g.line(cal.right() + 10.0, c.y + 2.0, cal.right() + 10.0, c.bottom() - 2.0, 1.0, WHITE.a(0.1));
        let c = R::new(cal.right() + 20.0, c.y, c.right() - cal.right() - 20.0, c.h);

        // input row: the text and the add button
        let field = R::new(c.x, c.y, c.w - 32.0 - 6.0, 30.0);
        let hv = ui.hit(field, Act::TaskField);
        g.rrect(field, 10.0, if self.input_focus { WHITE.a(0.12) } else { surf(g, hv) });
        if self.input_focus {
            g.rrect_stroke(field, 10.0, feat(g, Tab::Tasks).a(0.6), 1.2, false);
        }
        let inner = field.inset(12.0, 0.0);
        if self.input.is_empty() && !self.input_focus {
            g.text("Add a task…", inner, 13.0, 400, T3, Al::L);
        } else {
            let tw = g.measure(&self.input, 13.0, 400);
            let off = (tw - inner.w + 4.0).max(0.0);
            g.push_rr(inner, 0.0, 1.0);
            g.text(&self.input, R::new(inner.x - off, inner.y, tw + 50.0, inner.h), 13.0, 400, WHITE, Al::L);
            if self.input_focus && (self.t0.elapsed().as_millis() / 530) % 2 == 0 {
                let cx = inner.x - off + tw + 1.0;
                g.line(cx, inner.y + 8.0, cx, inner.bottom() - 8.0, 1.5, feat(g, Tab::Tasks));
            }
            g.pop();
        }
        let ar = R::new(c.right() - 32.0, c.y, 32.0, 30.0);
        let ah = ui.hit(ar, Act::TaskAdd);
        let ac = accent_btn(g, feat(g, Tab::Tasks));
        g.rrect(ar, 10.0, if ah { ac } else { ac.a(0.85) });
        g.icon(ic::ADD, ar, 12.0, accent_fg(g));

        // second row: the picked day, and the time a new task gets on it
        let hy = c.y + 36.0;
        let tr = R::new(c.right() - 96.0, hy, 96.0, 26.0);
        g.rrect(tr, 9.0, surf(g, false));
        let mr = R::new(tr.x, tr.y, 24.0, 26.0);
        let pr = R::new(tr.right() - 24.0, tr.y, 24.0, 26.0);
        let mid = R::new(tr.x + 24.0, tr.y, tr.w - 48.0, 26.0);
        let mh = ui.hit(mr, Act::TaskTime(-900));
        let ph = ui.hit(pr, Act::TaskTime(900));
        let th = ui.hit(mid, Act::Edit(Field::TaskTime));
        g.icon(ic::MINUS, mr, 9.0, if mh { WHITE } else { T2 });
        g.icon(ic::ADD, pr, 9.0, if ph { WHITE } else { T2 });
        match (self.edit_of(Field::TaskTime), self.draft_due()) {
            (Some(e), _) => value_box(g, mid, "", Some(e), false, 11.5),
            (None, Some(d)) => {
                let label = if d.div_euclid(86400) == self.sel_day { hm(d) } else { due_label(d, today) };
                g.text(&label, mid, 11.5, 600, if th { WHITE } else { feat(g, Tab::Tasks) }, Al::C);
            }
            (None, None) => g.text("No time", mid, 11.0, 500, if th { WHITE } else { T3 }, Al::C),
        }
        let sel = self.sel_day;
        let head = match sel - today {
            0 => format!("Today · {}", day_label(sel)),
            1 => format!("Tomorrow · {}", day_label(sel)),
            -1 => format!("Yesterday · {}", day_label(sel)),
            _ => {
                let (y, _, _) = civil_from_days(sel);
                if y == n.y {
                    day_label(sel)
                } else {
                    format!("{} {y}", day_label(sel))
                }
            }
        };
        let tasks = self.day_tasks();
        g.text(&head, R::new(c.x + 2.0, hy, tr.x - c.x - 8.0, 26.0), 11.5, 600, T2, Al::L);

        // list
        let ly = c.y + 68.0;
        let rh = 27.0;
        let rows = ((c.bottom() - ly) / rh).floor().max(1.0) as usize;
        if tasks.is_empty() {
            let y = ly + ((c.bottom() - ly - 40.0) / 2.0).max(0.0);
            let (title, hint) = if sel < today {
                ("Nothing was planned", "Pick today or a later day to add tasks.")
            } else if sel == today {
                ("No tasks for today", "Type a task, pick a time, press Enter.")
            } else {
                ("Nothing planned yet", "Tasks added now go on this day.")
            };
            g.text(title, R::new(c.x, y, c.w, 22.0), 13.5, 600, T2, Al::C);
            g.text(hint, R::new(c.x, y + 22.0, c.w, 18.0), 11.0, 400, T3, Al::C);
            return;
        }
        let start = self.task_scroll.min(tasks.len().saturating_sub(rows));
        for (i, t) in tasks.iter().skip(start).take(rows).enumerate() {
            let r = R::new(c.x, ly + i as f32 * rh, c.w, rh - 2.0);
            let hv = ui.hover(r);
            if hv {
                g.rrect(r, 9.0, WHITE.a(0.05));
            }
            let cb = R::new(r.x + 2.0, r.y, 28.0, r.h);
            let ch = ui.hit(cb, Act::TaskToggle(t.id));
            g.icon(if t.done { ic::BOX_ON } else { ic::BOX }, cb, 15.0, if t.done { feat(g, Tab::Tasks) } else if ch { WHITE } else { T2 });
            let overdue = !t.done && t.due.map(|d| d <= n.secs).unwrap_or(false);
            let label = t.due.map(|d| if d.div_euclid(86400) == sel { hm(d) } else { due_label(d, today) });
            let due_w = label.as_ref().map(|l| g.measure(l, 12.0, 600) + 6.0).unwrap_or(0.0);
            let txt = R::new(r.x + 36.0, r.y, r.w - 36.0 - due_w - 30.0, r.h);
            let off = self.marquee_text(g, t.id, hv, &t.text, txt, 13.0, 400, if t.done { T3 } else { WHITE });
            if t.done {
                let tw = g.measure(&t.text, 13.0, 400);
                let (x0, x1) = ((txt.x - off).max(txt.x), (txt.x - off + tw).min(txt.right()));
                g.line(x0, r.cy(), x1, r.cy(), 1.2, g.txt(T3));
            }
            if let Some(l) = &label {
                g.text(l, R::new(r.right() - due_w - 28.0, r.y, due_w, r.h), 12.0, 600, if t.done { T3 } else if overdue { RED } else { feat(g, Tab::Tasks) }, Al::R);
            }
            if hv {
                let xr = R::new(r.right() - 26.0, r.y + 3.0, 24.0, r.h - 6.0);
                let xh = ui.hit(xr, Act::TaskDel(t.id));
                g.icon(ic::CLOSE, xr, 10.0, if xh { RED } else { T3 });
            }
        }
        if tasks.len() > rows {
            g.text(&format!("{}–{} of {}", start + 1, (start + rows).min(tasks.len()), tasks.len()), R::new(c.x, c.bottom() - 2.0, c.w, 14.0), 10.0, 500, T3, Al::R);
        }
    }

    // -------------------------------------------------------------- actions

    fn cursor_inside(&self) -> bool {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        let (l, t, w, h) = self.isl_px;
        p.x >= l && p.x < l + w && p.y >= t && p.y < t + h
    }

    /// Whether the cursor is really over this window (not over its see-through pixels).
    fn cursor_on_window(&self) -> bool {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
            WindowFromPoint(p) == self.hwnd
        }
    }

    fn hit_at(&self, p: (f32, f32)) -> Option<Act> {
        self.hits.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, a)| *a)
    }

    fn act(&mut self, a: Act, mx: f32) {
        if let Some(e) = &self.edit {
            if a != Act::Edit(e.field) {
                self.commit_edit();
            }
        }
        if !matches!(a, Act::TaskField | Act::TaskTime(_) | Act::Edit(Field::TaskTime) | Act::TaskAdd | Act::CalMonth(_) | Act::CalToday | Act::CalDay(_)) {
            self.input_focus = false;
        }
        match a {
            Act::Tab(t) => {
                if t != Tab::Settings {
                    // the Colors page stays open: come back to Settings and it is still there
                    self.cancel_hk_capture();
                }
                self.shelf_sel.clear();
                if t != Tab::Shelf {
                    self.qr_pop = None;
                }
                self.tab = t;
                if t == Tab::Media && self.lyrics_open {
                    self.ensure_lyrics();
                }
            }
            Act::PlayPause => {
                self.send(MediaCmd::PlayPause);
                let p = self.media_pos();
                if let Some(m) = &mut self.media {
                    m.pos = p;
                    m.at = Instant::now();
                    m.playing = !m.playing;
                    self.pending_play = Some((m.playing, Instant::now()));
                }
            }
            Act::Prev => self.send(MediaCmd::Prev),
            Act::Next => self.send(MediaCmd::Next),
            Act::Seek(r) => {
                let f = ((mx - r.x) / r.w).clamp(0.0, 1.0) as f64;
                if let Some(m) = &mut self.media {
                    let t = f * m.dur;
                    m.pos = t;
                    m.at = Instant::now();
                    let _ = self.media_tx.send(MediaCmd::Seek(t));
                }
            }
            Act::Lyrics => {
                self.lyrics_open = !self.lyrics_open;
                if self.lyrics_open {
                    self.ensure_lyrics();
                    if let Some(c) = self.cur_line() {
                        self.lyr_scroll = c as f32;
                    }
                }
            }
            Act::MiniLyrics => self.toggle_mini_lyrics(),
            Act::Opt(o) => self.set_opt(o),
            Act::LyricLine(t) => {
                if let Some(m) = &mut self.media {
                    m.pos = t;
                    m.at = Instant::now();
                    let _ = self.media_tx.send(MediaCmd::Seek(t));
                }
            }
            Act::FocusToggle => {
                let n = Instant::now();
                match self.pomo.mode {
                    PMode::Idle => self.start_phase(PMode::Focus),
                    _ if self.pomo.paused => {
                        self.pomo.end = n + Duration::from_secs_f64(self.pomo.left);
                        self.pomo.paused = false;
                    }
                    _ => {
                        self.pomo.left = self.pomo.remaining();
                        self.pomo.paused = true;
                    }
                }
            }
            Act::FocusReset => self.pomo.mode = PMode::Idle,
            Act::FocusSkip => {
                if self.pomo.active() {
                    self.finish_phase(false);
                } else {
                    self.start_phase(PMode::Break);
                }
            }
            Act::FocusLen(brk, d) => {
                let f = &mut self.data.focus;
                if brk {
                    f.break_min = (f.break_min as i32 + d).clamp(1, 60) as u32;
                } else {
                    f.focus_min = (f.focus_min as i32 + d).clamp(5, 120) as u32;
                }
                self.dirty = true;
            }
            Act::WaterAdd(ml) => {
                let before = self.data.water.total();
                self.data.water.log.push(ml);
                self.water_mark = Instant::now();
                self.dirty = true;
                let w = &self.data.water;
                if before < w.goal_ml && w.total() >= w.goal_ml {
                    sound::chime();
                }
            }
            Act::WaterRemind => {
                let w = &mut self.data.water;
                w.remind = !w.remind;
                // turning them back on starts a fresh interval instead of pinging right away
                self.water_mark = Instant::now();
                self.dirty = true;
            }
            Act::WaterUndo => {
                self.data.water.log.pop();
                self.dirty = true;
            }
            Act::WaterSet(which, d) => {
                let w = &mut self.data.water;
                match which {
                    0 => w.bottle_ml = (w.bottle_ml as i32 + d).clamp(100, 2000) as u32,
                    1 => w.goal_ml = (w.goal_ml as i32 + d).clamp(500, 6000) as u32,
                    _ => w.interval_min = (w.interval_min as i32 + d).clamp(15, 240) as u32,
                }
                self.dirty = true;
            }
            Act::ShelfItem(i) => {
                // Ctrl+click, or any click while something is selected, toggles selection
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                if ctrl || !self.shelf_sel.is_empty() {
                    self.toggle_sel(i);
                } else if let Some(p) = self.data.shelf.get(i) {
                    shelf::open(p);
                }
            }
            Act::ShelfRemove(i) => {
                if let Some(p) = self.data.shelf.get(i) {
                    let p = p.clone();
                    self.shelf_sel.retain(|x| *x != p);
                }
                if i < self.data.shelf.len() {
                    self.data.shelf.remove(i);
                    self.dirty = true;
                }
            }
            // click selects/deselects everything; pressing and dragging it starts the drag directly
            Act::ShelfAll => {
                if self.shelf_sel.is_empty() {
                    self.shelf_sel = self.data.shelf.clone();
                } else {
                    self.shelf_sel.clear();
                }
            }
            Act::ShelfCheck(i) => self.toggle_sel(i),
            Act::QrBadge(i) => {
                if let Some(p) = self.data.shelf.get(i).cloned() {
                    self.qr_pop = Some(p);
                    self.qr_copied = None;
                }
            }
            Act::QrClose => self.qr_pop = None,
            Act::QrDo(i, a) => {
                let code = self.qr_pop.as_ref().and_then(|p| self.qr.get(p)?.as_ref()?.get(i).cloned());
                if let Some(code) = code {
                    if self.qr_run(&code, a) {
                        self.qr_copied = Some((Instant::now(), i));
                    }
                }
            }
            Act::ToastQr(a) => self.toast_qr(a),
            Act::ClipCopy(i) => {
                // copy it again but leave the list as it is: the entry keeps its place
                if let Some(t) = self.clips.get(i).map(|c| c.text.clone()) {
                    if clip::write(self.hwnd, &t) {
                        self.clip_ignore = Some(t.clone());
                        self.clip_copied = Some((Instant::now(), t));
                    }
                }
            }
            Act::ClipDel(i) => {
                if i < self.clips.len() {
                    self.clips.remove(i);
                }
            }
            // every entry, oldest first (the order they were copied in), one per line
            Act::ClipCopyAll => {
                if !self.clips.is_empty() {
                    let all = self.clips.iter().rev().map(|c| c.text.trim_end()).collect::<Vec<_>>().join("\r\n");
                    if clip::write(self.hwnd, &all) {
                        // the combined text must not come back as a new entry
                        self.clip_ignore = Some(all);
                        self.clip_all_at = Some(Instant::now());
                    }
                }
            }
            Act::ClipClear => {
                self.clips.clear();
                self.clip_scroll = 0;
            }
            Act::ShelfClear => {
                self.shelf_sel.clear();
                self.data.shelf.clear();
                self.shelf_scroll = 0;
                self.dirty = true;
            }
            Act::TaskField => {
                self.input_focus = true;
                unsafe {
                    let _ = SetForegroundWindow(self.hwnd);
                    let _ = SetFocus(self.hwnd);
                }
            }
            Act::TaskTime(d) => self.adjust_due(d),
            Act::Edit(f) => self.start_edit(f),
            Act::ColBack => self.colors_open = false,
            Act::SetCat(i) => {
                if i != 2 {
                    self.cancel_hk_capture();
                }
                self.settings_cat = i;
            }
            Act::ColTarget(t) => {
                self.col_target = t;
                self.col_hsv = theme::to_hsv(self.col_color());
            }
            Act::ColPreset(i) => self.col_preset(i),
            Act::ColCopy => self.col_copy(),
            Act::ColPaste => self.col_paste(),
            Act::ColSwatch(hx) => {
                if let Some(c) = theme::color(&theme::hex(C::hex(hx))) {
                    self.col_hsv = theme::to_hsv(c);
                    self.col_apply(c, true);
                }
            }
            Act::ColWheel(_) | Act::ColBright(_) => {}
            Act::ColAuto => {
                let kv: Vec<(&str, String)> = match self.col_target {
                    1 => vec![("expanded_mode", "same".into())],
                    2 => vec![("button_accent", "auto".into())],
                    COL_ICONS => vec![("icon", "auto".into())],
                    COL_TEXT => vec![("text", "auto".into()), ("expanded_text", "auto".into())],
                    n => vec![(theme::FEATURE_KEYS[(n as usize - 3).min(6)], "auto".into())],
                };
                self.col_set(&kv, true);
                self.col_hsv = theme::to_hsv(self.col_color());
            }
            Act::ColReset => {
                let all = theme::defaults();
                let kv: Vec<(&str, String)> = all.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                self.col_set(&kv, true);
                self.col_hsv = theme::to_hsv(self.col_color());
            }
            Act::TaskAdd => self.add_task(),
            Act::TaskToggle(id) => {
                if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
                    t.done = !t.done;
                    self.dirty = true;
                }
            }
            Act::TaskDel(id) => {
                self.data.tasks.retain(|t| t.id != id);
                self.dirty = true;
            }
            Act::CalMonth(d) => self.shift_month(d),
            Act::CalToday => self.select_day(now().day()),
            Act::CalDay(day) => self.select_day(day),
            Act::Toast => {
                if let Some(t) = self.toast.take() {
                    if !self.toast_tapped(t.tap) {
                        if let Some(tab) = t.tab {
                            self.expand(Some(tab));
                        }
                    }
                }
            }
            Act::VolBar(r) => {
                let f = ((mx - r.x) / r.w).clamp(0.0, 1.0);
                if let Some(a) = &self.audio {
                    a.set(f);
                }
            }
        }
        self.kick();
    }

    fn toggle_mini_lyrics(&mut self) {
        self.data.settings.mini_lyrics = !self.data.settings.mini_lyrics;
        self.dirty = true;
        if self.data.settings.mini_lyrics {
            self.ensure_lyrics();
            if let Some(c) = self.cur_line() {
                self.lyr_scroll = c as f32;
            }
        }
        self.kick();
    }

    fn adjust_due(&mut self, d: i64) {
        let n = now().secs;
        let round_up = |s: i64, step: i64| (s / step + 1) * step;
        let mut v = match self.draft_due() {
            Some(t) => t + d,
            None => round_up(n + 600, 900),
        };
        if v <= n {
            v = round_up(n, 300);
        }
        self.task_due = Some(v);
        self.follow_due();
    }

    /// The time a new task gets: the one picked, or 09:00 on a later day picked in the calendar.
    fn draft_due(&self) -> Option<i64> {
        match self.task_due {
            Some(t) => Some(t),
            None if self.sel_day > now().day() => Some(self.sel_day * 86400 + DEFAULT_TIME),
            None => None,
        }
    }

    /// The picked time moved to another day (+ past midnight, "30m" late in the evening):
    /// the calendar goes along.
    fn follow_due(&mut self) {
        if let Some(t) = self.task_due {
            let day = t.div_euclid(86400);
            if day != self.sel_day {
                self.sel_day = day;
                let (y, m, _) = civil_from_days(day);
                self.cal_month = (y, m);
                self.task_scroll = 0;
            }
        }
    }

    /// Pick a day in the calendar. A time already picked keeps its hour on the new day.
    fn select_day(&mut self, day: i64) {
        let n = now().secs;
        self.sel_day = day;
        let (y, m, _) = civil_from_days(day);
        self.cal_month = (y, m);
        self.task_scroll = 0;
        if let Some(t) = self.task_due {
            let v = day * 86400 + t.rem_euclid(86400);
            self.task_due = (v > n).then_some(v);
        }
    }

    fn shift_month(&mut self, d: i32) {
        let (y, m) = self.cal_month;
        let k = y * 12 + (m - 1) + d as i64;
        self.cal_month = (k.div_euclid(12), k.rem_euclid(12) + 1);
    }

    fn add_task(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            self.input_focus = true;
            return;
        }
        let n = now();
        let due = self.draft_due();
        self.data.next_id += 1;
        self.data.tasks.push(Task {
            id: self.data.next_id,
            text,
            due,
            done: false,
            notified: false,
            alerts: passed_alerts(due, n.secs),
        });
        self.input.clear();
        // on a later day the time stays, for adding more tasks to that day
        if self.sel_day <= n.day() {
            self.task_due = None;
        }
        self.dirty = true;
    }

    fn start_phase(&mut self, m: PMode) {
        let mins = if m == PMode::Break { self.data.focus.break_min } else { self.data.focus.focus_min };
        let secs = mins as f64 * 60.0;
        self.pomo = Pomo {
            mode: m,
            end: Instant::now() + Duration::from_secs_f64(secs),
            left: secs,
            total: secs,
            paused: false,
        };
    }

    fn finish_phase(&mut self, natural: bool) {
        match self.pomo.mode {
            PMode::Focus => {
                if natural {
                    self.data.focus.sessions += 1;
                    self.dirty = true;
                }
                self.start_phase(PMode::Break);
                if natural {
                    sound::chime();
                    let msg = format!("Time for a {}-minute break", self.data.focus.break_min);
                    self.toast(TIcon::G(ic::TIMER), "Focus session complete", &msg, 8.0, self.break_color(), Some(Tab::Focus));
                }
            }
            PMode::Break => {
                self.pomo.mode = PMode::Idle;
                if natural {
                    sound::chime();
                    self.toast(TIcon::G(ic::TIMER), "Break's over", "Ready for another focus session?", 8.0, self.focus_color(), Some(Tab::Focus));
                }
            }
            PMode::Idle => {}
        }
    }

    // -------------------------------------------------------------- periodic

    pub fn tick(&mut self) {
        let n = Instant::now();
        let mut redraw = false;

        if let Some(at) = self.clip_due {
            if n >= at {
                self.clip_due = None;
                self.read_clipboard();
            }
        }

        // new screenshots -> Shelf (one cheap folder check every 2 s)
        if self.data.settings.screenshots && n >= self.shots_next {
            self.shots_next = n + Duration::from_secs(2);
            let new: Vec<String> = self.shots.poll().into_iter().filter(|p| !self.data.shelf.contains(p)).collect();
            if !new.is_empty() {
                let count = new.len();
                for p in &new {
                    self.qr_scan(p, true);
                }
                self.data.shelf.extend(new);
                self.shelf_scroll = self.shelf_max();
                self.dirty = true;
                let title = if count == 1 { "Screenshot added to Shelf".to_string() } else { format!("{count} screenshots added to Shelf") };
                self.toast(TIcon::G(ic::FOLDER), &title, "Drag it anywhere from the Shelf", 4.0, feat(&self.gfx, Tab::Shelf), Some(Tab::Shelf));
                redraw = true;
            }
        }

        if let Some(t) = &self.toast {
            if n >= t.until && !self.cursor_inside() {
                self.toast = None;
                redraw = true;
            }
        }
        if !self.hidden && !self.expanded && self.toast.is_none() {
            if let Some(p) = self.qr_later.take() {
                self.qr_notice(&p);
                redraw = true;
            }
        }
        // once the island is small again, hand buffers and idle pages back to Windows
        let small = !self.expanded && self.toast.is_none() && self.leaving.is_none();
        if !small {
            self.trim_at = None;
        } else if self.trim_at.is_none() && self.gfx.oversized(self.win.2, self.win.3) {
            self.trim_at = Some(n + Duration::from_secs(3));
        }
        if let Some(at) = self.trim_at {
            if small && !self.anim_on && n >= at {
                self.trim_at = None;
                if self.win.2 > 0 && self.win.3 > 0 {
                    self.gfx.trim(self.win.2, self.win.3);
                }
                // previews come back (decoded again) the next time the Shelf opens
                self.thumbs.clear();
                crate::util::trim_memory();
                dbg_log("memory: trimmed");
                if std::env::var_os("VEYRA_DEBUG").is_some() {
                    dbg_log(&crate::util::heap_stats());
                }
            }
        }
        if self.pomo.active() && !self.pomo.paused && self.pomo.remaining() <= 0.0 {
            self.finish_phase(true);
            redraw = true;
        }

        if let Some((at, k)) = self.demo_stress {
            // VEYRA_STRESS_N: stop (closed) after that many steps, to watch memory settle
            let max = std::env::var("VEYRA_STRESS_N").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(u32::MAX);
            if k >= max && n >= at {
                self.demo_stress = None;
                dbg_log(&format!("stress done: {}", crate::util::heap_stats()));
                self.kick();
            } else if n >= at {
                // VEYRA_STRESS=fetch: only fetch lyrics again; =toggle: only close / reopen;
                // otherwise both (odd steps close, even steps reopen and fetch anew)
                let mode = std::env::var("VEYRA_STRESS").unwrap_or_default();
                if mode == "fetch" {
                    self.lyr_key = 0;
                    self.ensure_lyrics();
                } else if k % 2 == 0 {
                    self.collapse();
                } else if mode == "water" {
                    self.expand(Some(Tab::Water));
                } else {
                    self.lyrics_open = true;
                    if mode != "toggle" {
                        self.lyr_key = 0;
                    }
                    self.expand(Some(Tab::Media));
                    self.ensure_lyrics();
                }
                let ms = std::env::var("VEYRA_STRESS_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
                self.demo_stress = Some((n + Duration::from_millis(ms), k + 1));
                if k % 10 == 0 {
                    dbg_log(&format!("stress {k}: {}", crate::util::heap_stats()));
                }
                self.kick();
            }
        }

        if let Some((at, what)) = self.demo_step {
            if n >= at {
                self.demo_step = None;
                match what {
                    "found" => self.lyr = std::mem::replace(&mut self.demo_lyrics, Lyr::None),
                    "collapse" => self.collapse(),
                    "tocolors" => self.open_colors(),
                    "expand" => self.expand(Some(Tab::Media)),
                    "hover" => {
                        self.mouse = Some((self.screen_cx, 12.0));
                        self.hover_in = Some(n);
                        self.demo_step = Some((n + Duration::from_millis(700), "unhover"));
                    }
                    "unhover" => {
                        self.mouse = None;
                        self.hover_out = Some(n);
                    }
                    _ => {}
                }
                self.kick();
            }
        }

        if let Some(at) = self.demo_switch {
            if n >= at {
                self.demo_switch = None;
                self.tab = Tab::Focus;
                self.kick();
            }
        }

        // keyboard layout
        let l = system::fg_lang();
        if l != 0 && l != self.lang {
            let first = self.lang == 0;
            self.lang = l;
            if !first {
                let (name, short) = system::lang_name(l);
                self.toast(TIcon::Txt(short), &name, "Keyboard language", 2.2, WHITE, None);
            }
        }

        if n.duration_since(self.last_sec) >= Duration::from_secs(1) {
            self.last_sec = n;
            self.per_second();
        }

        if let Some(at) = self.net_check {
            if n >= at {
                self.net_check = None;
                let (ok, name, wifi) = system::net_status();
                if self.net != Some(ok) {
                    self.net = Some(ok);
                    if ok {
                        let ch = if wifi { ic::WIFI } else { ic::ETHERNET };
                        let title = if wifi { "Wi-Fi connected" } else { "Connected" };
                        self.toast(TIcon::G(ch), title, &name, 3.0, GREEN, None);
                    } else {
                        self.toast(TIcon::G(ic::WIFI), "Connection lost", "You're offline", 4.0, RED, None);
                    }
                }
            }
        }

        // a resize under a still cursor can produce a spurious WM_MOUSELEAVE: recover hover
        // (not while it is a line: it opens only on a real mouse movement over the line, and
        // the island rectangle may still be the size of the panel that just closed)
        let settling = self.no_hover_until.map(|t| n < t).unwrap_or(false);
        if self.mouse.is_none()
            && !self.hidden
            && self.view != ViewKey::Line
            && !settling
            && self.cursor_inside()
            && self.cursor_on_window()
        {
            let mut p = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
            let s = self.gfx.scale;
            self.mouse = Some(((p.x - self.win.0) as f32 / s, (p.y - self.win.1) as f32 / s));
            self.tracking = false;
            self.track_leave();
            redraw = true;
        }

        // the other way round: a WM_MOUSELEAVE can get lost (a full-screen game taking the
        // mouse, for one), and then nothing would ever end the hover
        if self.mouse.is_some() && !self.demo && !self.hidden && !self.cursor_on_window() {
            let held = unsafe { GetCapture() } == self.hwnd;
            if !held {
                dbg_log("hover: lost WM_MOUSELEAVE, ending it");
                self.on_mouse_leave();
            }
        }

        // auto-collapse when the island is open but the user went elsewhere
        if self.expanded && !self.demo && !self.menu_hold {
            self.arm_auto_close(n);
            if let Some(at) = self.collapse_at {
                if n >= at && !self.cursor_inside() && !self.drag_hover {
                    self.collapse();
                }
            }
        }

        let playing = self.media.as_ref().map(|m| m.playing).unwrap_or(false);
        let compact = !self.expanded && self.toast.is_none();
        // redraw when the closed island should switch activity (e.g. a task became due soon)
        // or, for the idle clock, when the minute rolls over
        let minute = now().secs / 60;
        if minute != self.last_min || (compact && self.target().3 != self.view) {
            self.last_min = minute;
            redraw = true;
        }
        let periodic = (compact && (playing || self.pomo.active() || self.task_soon().is_some()))
            || (self.expanded
                && match self.tab {
                    Tab::Media => playing,
                    Tab::Focus => self.pomo.active(),
                    Tab::Tasks => self.input_focus || self.edit.is_some(),
                    _ if self.edit.is_some() => true,
                    _ => false,
                });
        let fast = (compact && playing) || (self.expanded && self.tab == Tab::Media && self.lyrics_open && playing);
        // quicker while a panel is about to close, so it closes on time
        self.set_tick(if fast || (self.expanded && self.collapse_at.is_some()) { 90 } else { 250 });
        if (periodic || redraw) && !self.hidden {
            self.kick();
        }
    }

    fn per_second(&mut self) {
        // hide during full-screen apps / presentations; reminders wait until it is back
        self.apply_visibility();
        self.poll_privacy();
        let t = now();
        let today = t.date();
        if self.data.water.date != today {
            self.data.water.date = today.clone();
            self.data.water.log.clear();
            self.dirty = true;
        }
        if self.data.focus.date != today {
            self.data.focus.date = today;
            self.data.focus.sessions = 0;
            self.dirty = true;
        }

        // water reminder (07:00–22:59)
        let w = &self.data.water;
        let iv = Duration::from_secs(w.interval_min as u64 * 60);
        if w.remind && !self.hidden && (7..23).contains(&t.h) && w.total() < w.goal_ml && self.water_mark.elapsed() >= iv {
            self.water_mark = Instant::now();
            let msg = format!("{} / {} ml today · tap to log", w.total(), w.goal_ml);
            sound::chime();
            self.toast(TIcon::Drop, "Time to drink some water", &msg, 10.0, feat(&self.gfx, Tab::Water), Some(Tab::Water));
        }

        // task reminders: 1 day, 6 h, 3 h, 1 h and 30 min before, and when due. Not while
        // hidden (full-screen game, video or presentation): they come once it is back.
        if !self.hidden {
            self.task_reminders(t.secs);
        }

        // updates: a quiet check now and then, the offer once nothing is in the way
        if !self.demo && !self.upd_busy && Instant::now() >= self.upd_next && std::env::var_os("VEYRA_NO_UPDATE").is_none() {
            self.upd_next = Instant::now() + Duration::from_secs(6 * 3600);
            self.upd_busy = true;
            update::check(false);
        }
        self.offer_update();

        // theme.json edited: apply the new colors right away
        let m = theme::modified();
        if m != self.theme_mtime {
            self.theme_mtime = m;
            self.reload_theme();
        }

        if self.dirty {
            self.save_now();
        }
    }

    fn task_reminders(&mut self, now_s: i64) {
        let today = now_s.div_euclid(86400);
        let mut due: Vec<String> = Vec::new();
        // (task text, deadline)
        let mut soon: Vec<(String, i64)> = Vec::new();
        for task in self.data.tasks.iter_mut() {
            let Some(d) = task.due else { continue };
            if task.done {
                continue;
            }
            let (which, bits) = reminder_due(d, now_s, task.alerts);
            if bits != task.alerts {
                task.alerts = bits;
                self.dirty = true;
            }
            if which.is_some() {
                soon.push((task.text.clone(), d));
            }
            if !task.notified && d <= now_s {
                task.notified = true;
                self.dirty = true;
                due.push(task.text.clone());
            }
        }
        if !due.is_empty() {
            sound::chime();
            let sub = if due.len() == 1 { due[0].clone() } else { format!("{} and {} more", due[0], due.len() - 1) };
            self.toast(TIcon::G(ic::TASKS), "Task due now", &sub, 12.0, feat(&self.gfx, Tab::Tasks), Some(Tab::Tasks));
        } else if !soon.is_empty() {
            // the most pressing one leads
            soon.sort_by_key(|(_, d)| *d);
            let (text, d) = &soon[0];
            let when = if d.div_euclid(86400) == today { hm(*d) } else { due_label(*d, today) };
            let (title, sub) = if soon.len() == 1 {
                (format!("Task due {}", left_label(d - now_s)), format!("{text} · {when}"))
            } else {
                (format!("{} tasks due soon", soon.len()), format!("{text} · {when} and {} more", soon.len() - 1))
            };
            sound::chime();
            self.toast(TIcon::G(ic::TASKS), &title, &sub, 12.0, feat(&self.gfx, Tab::Tasks), Some(Tab::Tasks));
        }
    }

    // -------------------------------------------------------------- events

    pub fn on_events(&mut self) {
        for e in events::drain() {
            match e {
                Event::Media(mut m) => {
                    dbg_log(&format!("app: media event -> {}", m.as_ref().map(|m| m.title.as_str()).unwrap_or("<none>")));
                    if self.demo_idle {
                        m = None;
                    }
                    // the made-up track of the mini lyrics demo stays
                    if self.demo && self.lyr_key == DEMO_KEY {
                        continue;
                    }
                    let old = self.media.as_ref().map(|m| m.key);
                    let new = m.as_ref().map(|m| m.key);
                    // Keep the optimistic play/pause state until the player confirms it,
                    // otherwise a stale update makes the icon flicker back for a moment.
                    if let (Some(st), Some((want, since))) = (m.as_mut(), self.pending_play) {
                        if st.playing == want || since.elapsed() > Duration::from_millis(2500) || new != old {
                            self.pending_play = None;
                        } else {
                            st.playing = want;
                        }
                    }
                    self.media = m;
                    if self.media.is_none() {
                        self.pending_play = None;
                        if self.expanded && self.tab == Tab::Media {
                            self.lyrics_open = false;
                        }
                    }
                    if new != old && (self.lyrics_open || self.data.settings.mini_lyrics) {
                        self.ensure_lyrics();
                    }
                }
                Event::Art(a) => match a {
                    // Art arrives before the matching Media event, so accept it and tag it
                    // with its track key; it is only shown while that track is current.
                    Some(a) => {
                        self.art = self.gfx.create_bitmap(a.w, a.h, &a.px);
                        self.art_key = a.key;
                        self.art_color = C(a.color.0, a.color.1, a.color.2, 1.0);
                    }
                    None => {
                        self.art = None;
                        self.art_key = 0;
                    }
                },
                Event::Lyrics(key, r) => {
                    // the download's buffers (JSON with every search hit) are freed by now
                    crate::util::release_free_heap();
                    if key == self.lyr_key {
                        self.lyr = match r {
                            Some(v) if !v.is_empty() => Lyr::Found(v),
                            _ => Lyr::Missing,
                        };
                        if let Some(c) = self.cur_line() {
                            self.lyr_scroll = c as f32;
                        }
                    }
                }
                Event::Volume(v, muted) => {
                    let ch = if muted || v <= 0.001 { ic::MUTE } else { ic::VOL };
                    self.hud(K_VOL, ch, if muted { 0.0 } else { v }, WHITE);
                }
                Event::AudioDevice => {
                    let changed = self.audio.as_mut().map(|a| a.rebind()).unwrap_or(false);
                    if changed {
                        let name = self.audio.as_ref().map(|a| a.name.clone()).unwrap_or_default();
                        let l = name.to_lowercase();
                        let wireless = ["bluetooth", "buds", "airpods", "headphone", "headset", "earbud", "wh-", "wf-"]
                            .iter()
                            .any(|k| l.contains(k));
                        let title = if wireless { "Headphones connected" } else { "Audio output changed" };
                        self.toast(TIcon::G(ic::HEADPHONE), title, &name, 3.5, WHITE, None);
                    }
                }
                Event::NetChanged => self.net_check = Some(Instant::now() + Duration::from_millis(1500)),
                Event::Caps => {
                    self.caps = !self.caps;
                    let title = if self.caps { "Caps Lock on" } else { "Caps Lock off" };
                    self.toast(TIcon::Txt("⇪".into()), title, "", 1.4, if self.caps { GREEN } else { WHITE }, None);
                }
                Event::DragEnter(text) => {
                    self.drag_hover = true;
                    self.drag_text = text;
                    self.collapse_at = None;
                    if !self.expanded {
                        self.expanded = true;
                        self.drag_expanded = true;
                        self.panel_entered = false;
                        system::watch_clicks(true);
                        self.toast = None;
                    }
                    self.tab = if text { Tab::Clipboard } else { Tab::Shelf };
                }
                Event::MouseDown => {
                    // opened by a drag, so not the active window: a click elsewhere closes it;
                    // and in click-to-close mode any click outside the open island does
                    let watching = self.drag_expanded || self.data.settings.close_on_click;
                    if self.expanded && watching && !self.drag_hover && !self.menu_hold && !self.cursor_inside() {
                        self.collapse();
                    }
                }
                Event::DragLeave => {
                    self.drag_hover = false;
                    if self.drag_expanded {
                        self.collapse_at = Some(Instant::now() + Duration::from_millis(700));
                    }
                }
                Event::Drop(files) => {
                    self.drag_hover = false;
                    for f in files {
                        if !self.data.shelf.contains(&f) {
                            self.qr_scan(&f, true);
                            self.data.shelf.push(f);
                        }
                    }
                    // slide to the end so the newly added files come into view
                    self.shelf_scroll = self.shelf_max();
                    self.dirty = true;
                    if self.drag_expanded {
                        self.collapse_at = Some(Instant::now() + Duration::from_millis(2500));
                    }
                }
                Event::DropText(t) => {
                    self.drag_hover = false;
                    self.drag_text = false;
                    // kept in the history only: what is on the Windows clipboard stays as it is
                    if clip::push(&mut self.clips, t, now().secs) {
                        self.clip_scroll = 0;
                        self.clip_dropped = self.clips.first().map(|i| (Instant::now(), i.text.clone()));
                        dbg_log(&format!("clipboard: {} entries (dropped)", self.clips.len()));
                    }
                    if self.drag_expanded {
                        self.collapse_at = Some(Instant::now() + Duration::from_millis(2500));
                    }
                }
                Event::UpdateInfo(r, manual) => self.on_update_info(r, manual),
                Event::UpdateDone(r) => self.on_update_done(r),
                Event::Qr(p, codes, notify) => self.on_qr(p, codes, notify),
                Event::Thumb(p, t) => {
                    // only while the file is still on the Shelf (it may be gone by now)
                    if let Some(slot) = self.thumbs.get_mut(&p) {
                        *slot = t.and_then(|t| self.gfx.create_bitmap(t.w, t.h, &t.px));
                    }
                }
            }
        }
        self.kick();
    }

    pub fn on_power(&mut self, wp: WPARAM, lp: LPARAM) {
        const PBT_APMPOWERSTATUSCHANGE: usize = 0x000A;
        const PBT_POWERSETTINGCHANGE: usize = 0x8013;
        match wp.0 {
            PBT_APMPOWERSTATUSCHANGE => {
                let p = system::power();
                if let (Some((ac, pct)), Some((old_ac, _))) = (p, self.power) {
                    if ac != old_ac {
                        if ac {
                            self.low_warned = false;
                            self.toast(TIcon::Battery(pct as f32 / 100.0, true), "Charging", &format!("{pct}% · plugged in"), 3.0, GREEN, None);
                        } else {
                            self.toast(TIcon::Battery(pct as f32 / 100.0, false), "On battery", &format!("{pct}% remaining"), 3.0, WHITE, None);
                        }
                    } else if !ac && pct <= 20 && !self.low_warned {
                        self.low_warned = true;
                        self.toast(TIcon::Battery(pct as f32 / 100.0, false), "Battery low", &format!("{pct}% left — plug in soon"), 6.0, RED, None);
                    }
                }
                self.power = p;
                self.kick();
            }
            PBT_POWERSETTINGCHANGE => unsafe {
                let s = &*(lp.0 as *const POWERBROADCAST_SETTING);
                if s.PowerSetting == system::GUID_BRIGHTNESS && s.DataLength >= 4 {
                    let v = std::ptr::read_unaligned(s.Data.as_ptr() as *const u32);
                    if self.bright_seen {
                        self.hud(K_BRIGHT, ic::BRIGHT, (v.min(100)) as f32 / 100.0, C::hex(0xFFD60A));
                    }
                    self.bright_seen = true;
                }
            },
            _ => {}
        }
    }

    pub fn on_display_change(&mut self) {
        let (cx, top, s) = monitor();
        self.mon_cx = cx;
        self.mon_top = top;
        crate::vsync::display_changed();
        if (s - self.gfx.scale).abs() > 0.001 {
            self.gfx.set_scale(s);
        }
        self.kick();
    }

    // -------------------------------------------------------------- input

    fn local(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xFFFF) as i16 as f32;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
        (x / self.gfx.scale, y / self.gfx.scale)
    }

    /// Ask Windows for a WM_MOUSELEAVE, once per stay of the cursor on the window.
    fn track_leave(&mut self) {
        if !self.tracking {
            self.tracking = true;
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
        }
    }

    pub fn on_mouse_move(&mut self, lp: LPARAM) {
        let p = self.local(lp);
        if self.mouse.is_none() {
            self.hover_in = Some(Instant::now());
        }
        self.track_leave();
        self.mouse = Some(p);
        if self.expanded {
            self.panel_entered = true;
        }
        if let Some((Some(a @ (Act::ColWheel(_) | Act::ColBright(_))), _, _)) = self.press {
            self.col_drag(a, p, false);
        }
        if let Some((Some(a), sx, sy)) = self.press {
            if (p.0 - sx).abs() + (p.1 - sy).abs() > 6.0 {
                let paths: Vec<String> = match a {
                    // dragging a selected tile (or the button) carries the selection, in shelf order
                    Act::ShelfItem(i) if self.data.shelf.get(i).map(|p| self.shelf_sel.contains(p)).unwrap_or(false) => self.selected_paths(),
                    Act::ShelfItem(i) => self.data.shelf.get(i).cloned().into_iter().collect(),
                    Act::ShelfAll if !self.shelf_sel.is_empty() => self.selected_paths(),
                    Act::ShelfAll => self.data.shelf.clone(),
                    _ => Vec::new(),
                };
                if matches!(a, Act::ShelfItem(_) | Act::ShelfAll) {
                    self.press = None;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    if !paths.is_empty() {
                        self.shelf_sel.clear();
                        self.deferred = Some(Deferred::DragOut(paths));
                    }
                }
            }
        }
        self.kick();
    }

    /// Decides whether (and when) the open panel should close by itself.
    fn arm_auto_close(&mut self, n: Instant) {
        if self.cursor_inside() {
            self.panel_entered = true;
            self.collapse_at = None;
            return;
        }
        if self.data.settings.close_on_click && !self.drag_expanded {
            // stays open until a click outside it (Event::MouseDown, deactivation) or Esc
            self.collapse_at = None;
            return;
        }
        // typing, or holding the mouse button (dragging the seek bar): stay until that is over
        let busy = self.input_focus || self.edit.is_some() || self.press.is_some();
        let fg = unsafe { GetForegroundWindow() };
        if self.collapse_at.is_none() {
            if self.drag_expanded {
                // after the drop the cursor came back over the island and has now left it: the
                // file is on the Shelf, so close about as soon as the drag leaving would
                let wait = if self.panel_entered { 700 } else { 2500 };
                self.collapse_at = Some(n + Duration::from_millis(wait));
            } else if self.panel_entered && !busy {
                self.collapse_at = Some(n + PANEL_OUT);
            } else if fg != self.hwnd {
                self.collapse_at = Some(n + Duration::from_millis(1200));
            }
        } else if busy && fg == self.hwnd && !self.drag_expanded {
            self.collapse_at = None;
        }
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking = false;
        if self.menu_hold {
            // the island's own menu took the mouse: settled when the menu closes (menu_closed)
            return;
        }
        self.mouse = None;
        self.hover_out = Some(Instant::now());
        if self.expanded && !self.demo {
            self.arm_auto_close(Instant::now());
            if self.collapse_at.is_some() {
                self.set_tick(90);
            }
        }
        self.kick();
    }

    /// Everything fires on release (like a normal button): pressing only remembers what is
    /// under the cursor, and the action runs when the button is released over the same thing.
    /// Holding the mouse down therefore never opens the island or triggers a button, and
    /// sliding off before releasing cancels. Shelf tiles can be dragged out while held.
    pub fn on_ldown(&mut self, lp: LPARAM) {
        let p = self.local(lp);
        self.mouse = Some(p);
        let hit = self.hit_at(p);
        self.press = Some((hit, p.0, p.1));
        unsafe {
            SetCapture(self.hwnd);
        }
        if let Some(a @ (Act::ColWheel(_) | Act::ColBright(_))) = hit {
            self.commit_edit();
            self.col_drag(a, p, false);
        }
    }

    pub fn on_lup(&mut self, lp: LPARAM) {
        let p = self.local(lp);
        unsafe {
            let _ = ReleaseCapture();
        }
        let Some((hit, _, _)) = self.press.take() else { return };
        // the color wheel / brightness bar: wherever the mouse is let go, that color stays
        if let Some(a @ (Act::ColWheel(_) | Act::ColBright(_))) = hit {
            self.col_drag(a, p, true);
            return;
        }
        // released somewhere else than where it was pressed: cancel
        if !self.cursor_inside() || self.hit_at(p) != hit {
            return;
        }
        if !self.expanded {
            match hit {
                Some(Act::VolBar(r)) => self.act(Act::VolBar(r), p.0),
                Some(Act::ToastQr(a)) => self.toast_qr(a),
                Some(Act::Tab(t)) => self.expand(Some(t)),
                Some(Act::Toast) => {
                    let (tab, tap) = self.toast.as_ref().map(|t| (t.tab, t.tap)).unwrap_or((None, Tap::None));
                    self.toast = None;
                    if !self.toast_tapped(tap) {
                        self.expand(tab);
                    }
                }
                _ => self.expand(None),
            }
            return;
        }
        match hit {
            Some(a) => self.act(a, p.0),
            None => {
                self.input_focus = false;
                self.commit_edit();
                self.kick();
            }
        }
    }

    pub fn on_rup(&mut self) {
        self.menu_hold = true;
        self.deferred = Some(Deferred::Menu);
    }

    /// The menu is gone: back to what the mouse is really doing.
    pub fn menu_closed(&mut self) {
        if !self.menu_hold {
            return;
        }
        self.menu_hold = false;
        self.tracking = false;
        if self.cursor_inside() && self.cursor_on_window() {
            let mut p = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
            let s = self.gfx.scale;
            self.mouse = Some(((p.x - self.win.0) as f32 / s, (p.y - self.win.1) as f32 / s));
            self.track_leave();
            self.kick();
        } else {
            self.on_mouse_leave();
        }
    }

    pub fn on_wheel(&mut self, wp: WPARAM, lp: LPARAM) {
        let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
        let steps = delta.signum();
        let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        let p = (pt.x as f32 / self.gfx.scale, pt.y as f32 / self.gfx.scale);
        let hit = self.hit_at(p);
        let vol_step = |s: &mut App| {
            if let Some(a) = &s.audio {
                if let Some((v, _)) = a.get() {
                    a.set(((v * 50.0).round() + steps as f32) / 50.0);
                }
            }
        };
        if !self.expanded {
            vol_step(self);
            return;
        }
        match (self.tab, hit) {
            (Tab::Tasks, Some(Act::CalDay(_) | Act::CalMonth(_) | Act::CalToday)) => self.shift_month(-steps),
            (Tab::Tasks, Some(Act::TaskTime(_))) | (Tab::Tasks, Some(Act::Edit(Field::TaskTime))) => {
                if self.task_due.is_none() {
                    self.adjust_due(0);
                } else {
                    self.adjust_due(steps as i64 * 300);
                }
            }
            (Tab::Tasks, _) => {
                if steps < 0 {
                    self.task_scroll = (self.task_scroll + 1).min(self.data.tasks.len().saturating_sub(1));
                } else {
                    self.task_scroll = self.task_scroll.saturating_sub(1);
                }
            }
            (Tab::Clipboard, _) => {
                if steps < 0 {
                    self.clip_scroll = (self.clip_scroll + 1).min(self.clips.len().saturating_sub(1));
                } else {
                    self.clip_scroll = self.clip_scroll.saturating_sub(1);
                }
            }
            (Tab::Shelf, _) => {
                if steps < 0 {
                    self.shelf_scroll = (self.shelf_scroll + 1).min(self.shelf_max());
                } else {
                    self.shelf_scroll = self.shelf_scroll.saturating_sub(1);
                }
            }
            (Tab::Media, _) if !self.lyrics_open => vol_step(self),
            _ => {}
        }
        self.kick();
    }

    pub fn on_activate(&mut self, active: bool) {
        if !active {
            self.input_focus = false;
            if self.expanded && !self.drag_expanded && !self.demo && !self.menu_hold && !self.cursor_inside() {
                dbg_log("collapse: deactivate");
                self.collapse();
            }
        }
        self.kick();
    }

    pub fn on_key(&mut self, vk: u32) -> bool {
        if self.hk_capture && self.expanded && self.tab == Tab::Settings {
            self.hk_key(vk);
            return true;
        }
        if vk == VK_ESCAPE.0 as u32 {
            if self.edit.is_some() {
                self.edit = None;
            } else if self.input_focus {
                self.input_focus = false;
            } else if self.expanded && self.colors_open && self.tab == Tab::Settings {
                self.colors_open = false;
            } else if self.expanded {
                self.collapse();
            }
            self.kick();
            return true;
        }
        false
    }

    pub fn on_char(&mut self, ch: u32) {
        let on_colors = self.expanded && self.tab == Tab::Settings && self.colors_open;
        if on_colors && (self.edit.is_none() || self.edit.as_ref().map(|e| e.field) == Some(Field::Hex)) {
            match ch {
                0x03 | 0x18 => return self.col_copy(),
                0x16 => return self.col_paste(),
                _ => {}
            }
        }
        if let Some(e) = &mut self.edit {
            match ch {
                0x08 => {
                    if e.fresh {
                        e.buf.clear();
                    } else {
                        e.buf.pop();
                    }
                    e.fresh = false;
                }
                0x0D => self.commit_edit(),
                _ => {
                    if let Some(c) = char::from_u32(ch) {
                        let c = c.to_ascii_lowercase();
                        let hex = e.field == Field::Hex;
                        let ok = if hex { c.is_ascii_hexdigit() || c == '#' } else { c.is_ascii_digit() || ":.,hml ".contains(c) };
                        if ok {
                            if e.fresh {
                                e.buf.clear();
                                e.fresh = false;
                            }
                            if e.buf.len() < if hex { 7 } else { 10 } {
                                e.buf.push(c);
                            }
                        }
                    }
                }
            }
            self.kick();
            return;
        }
        if !self.input_focus {
            return;
        }
        let ch = ch as u16;
        match ch {
            0x08 => {
                self.input.pop();
            }
            0x0D => self.add_task(),
            0x16 => {
                if let Some(s) = clipboard_text() {
                    let s: String = s.chars().filter(|c| !c.is_control()).collect();
                    self.input.push_str(&s);
                }
            }
            0x01..=0x1F | 0x7F => {}
            0xD800..=0xDBFF => self.surrogate = Some(ch),
            0xDC00..=0xDFFF => {
                if let Some(hi) = self.surrogate.take() {
                    if let Some(Ok(c)) = char::decode_utf16([hi, ch]).next() {
                        self.input.push(c);
                    }
                }
            }
            _ => {
                if let Some(c) = char::from_u32(ch as u32) {
                    self.input.push(c);
                }
            }
        }
        if self.input.chars().count() > 140 {
            self.input = self.input.chars().take(140).collect();
        }
        self.kick();
    }

    fn reload_theme(&mut self) {
        if self.colors_open {
            // theme.conf edited by hand while the Colors page is open: keep working on that
            self.col_text = theme::read_text();
        }
        let th = theme::load();
        self.gfx.set_theme(&th);
        dbg_log("theme reloaded");
        self.kick();
        if !self.hidden {
            self.render();
        }
    }

    pub fn menu_state(&self) -> tray::MenuState {
        tray::MenuState {
            hidden: self.user_hidden,
            update: self.upd_avail.as_ref().map(|r| r.version.clone()),
            update_busy: self.upd_busy,
        }
    }

    // -------------------------------------------------------------- updates

    /// A notice that does something of its own when tapped. Returns true if it did.
    fn toast_tapped(&mut self, tap: Tap) -> bool {
        match tap {
            Tap::None => false,
            Tap::Update => {
                self.start_update();
                true
            }
            Tap::Releases => {
                update::open_releases_page();
                true
            }
            Tap::Qr => {
                if let Some((_, p)) = self.qr_toast.take() {
                    self.show_qr_page(&p);
                }
                true
            }
        }
    }

    fn notice(&mut self, icon: TIcon, title: &str, sub: &str, secs: f32, accent: C, tap: Tap) {
        self.toast(icon, title, sub, secs, accent, None);
        if let Some(t) = &mut self.toast {
            t.tap = tap;
        }
    }

    /// Tray item: check now, or install the update already found.
    fn update_cmd(&mut self) {
        if self.upd_busy {
            return;
        }
        if self.upd_avail.is_some() {
            self.start_update();
        } else {
            self.upd_busy = true;
            self.upd_manual = true;
            self.upd_next = Instant::now() + Duration::from_secs(6 * 3600);
            update::check(true);
            self.notice(TIcon::G(ic::DOWNLOAD), "Checking for updates…", "", 4.0, WHITE, Tap::None);
        }
    }

    fn start_update(&mut self) {
        let Some(r) = self.upd_avail.clone() else { return };
        if self.upd_busy {
            return;
        }
        self.upd_busy = true;
        if self.expanded {
            self.collapse();
        }
        let sub = format!("Downloading v{} · Veyra restarts by itself", r.version);
        update::install(r);
        self.notice(TIcon::G(ic::DOWNLOAD), "Updating Veyra…", &sub, 120.0, GREEN, Tap::None);
    }

    fn on_update_info(&mut self, r: Result<Option<update::Release>, ()>, manual: bool) {
        self.upd_busy = false;
        let manual = manual || std::mem::take(&mut self.upd_manual);
        match r {
            Ok(Some(rel)) => {
                dbg_log(&format!("update: v{} available", rel.version));
                self.upd_avail = Some(rel);
                if self.demo && std::env::var("VEYRA_DEMO").as_deref() == Ok("update") {
                    self.start_update();
                    return;
                }
                if manual {
                    // asked for it: offer it again even if it was offered before
                    self.upd_shown = None;
                    self.toast = None;
                }
                self.offer_update();
            }
            Ok(None) => {
                self.upd_avail = None;
                if manual {
                    self.notice(TIcon::G(ic::CHECK), "Veyra is up to date", &format!("Version {}", update::CURRENT), 3.0, GREEN, Tap::None);
                }
            }
            Err(()) => {
                dbg_log("update: check failed");
                if manual {
                    self.notice(TIcon::G(ic::WIFI), "Couldn't check for updates", "No connection to GitHub", 4.0, RED, Tap::None);
                }
            }
        }
    }

    /// "Update available" once per version, and not while hidden (full screen) or open.
    fn offer_update(&mut self) {
        let Some(r) = &self.upd_avail else { return };
        if self.upd_busy || self.hidden || self.expanded || self.upd_shown.as_deref() == Some(r.version.as_str()) {
            return;
        }
        // don't push away another notice; per_second tries again
        if self.toast.is_some() {
            return;
        }
        let v = r.version.clone();
        self.upd_shown = Some(v.clone());
        self.notice(TIcon::G(ic::DOWNLOAD), &format!("Veyra {v} is available"), "Tap to update · takes a few seconds", 12.0, GREEN, Tap::Update);
    }

    fn on_update_done(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => {
                dbg_log("update: installed, restarting");
                self.save_now();
                if update::relaunch() {
                    tray::remove(self.hwnd);
                    crate::quit_now();
                }
                // could not start the new copy: it runs from the next launch on
                self.upd_busy = false;
                self.upd_avail = None;
                self.notice(TIcon::G(ic::CHECK), "Update installed", "Restart Veyra to use the new version", 8.0, GREEN, Tap::None);
            }
            Err(e) => {
                dbg_log(&format!("update: failed: {e}"));
                self.upd_busy = false;
                self.notice(TIcon::G(ic::DOWNLOAD), "Update failed", &format!("{e} · tap to open the download page"), 10.0, RED, Tap::Releases);
            }
        }
    }

    /// Started by the updater (`--after-update`).
    pub fn updated(&mut self) {
        self.notice(TIcon::G(ic::CHECK), "Veyra updated", &format!("You're on version {}", update::CURRENT), 5.0, GREEN, Tap::None);
    }

    /// The clipboard changed. Reading it right away would hold it open while the copying app
    /// may still be finishing its copy (OLE apps flush after announcing it), making that app's
    /// copy fail now and then; so read a moment later, from tick(). This also merges the two
    /// notifications many apps send per copy.
    pub fn on_clipboard(&mut self) {
        if self.clip_due.is_none() {
            self.clip_due = Some(Instant::now() + Duration::from_millis(150));
        }
    }

    /// Remember a text copy (see clip.rs for what is skipped).
    fn read_clipboard(&mut self) {
        if let Some(t) = clip::read(self.hwnd) {
            // the echo of an entry copied from the list itself
            if self.clip_ignore.take().as_deref() == Some(t.as_str()) {
                return;
            }
            let blank = t.trim().is_empty();
            let added = clip::push(&mut self.clips, t, now().secs);
            if added {
                self.clip_scroll = 0;
                // count only: copied text must never reach the log file
                dbg_log(&format!("clipboard: {} entries", self.clips.len()));
                if self.expanded && self.tab == Tab::Clipboard && !self.hidden {
                    self.render();
                }
            }
            // copying the same text again is a copy too, but some apps announce one copy twice
            let again = self.copied_at.map(|t| t.elapsed() >= Duration::from_secs(1)).unwrap_or(true);
            if !blank && (added || again) {
                self.copied_at = Some(Instant::now());
                self.copied_notice();
            }
        }
    }

    /// A small pill. Not over the open panel, and not over a reminder that can be tapped.
    fn copied_notice(&mut self) {
        let reminder = self.toast.as_ref().map(|t| t.tab.is_some() || t.tap != Tap::None).unwrap_or(false);
        if !self.expanded && !reminder && !self.demo {
            self.toast(TIcon::G(ic::CLIP), "Text copied", "", 1.4, WHITE, None);
        }
    }

    // -------------------------------------------------------------- QR codes

    /// Look for QR codes in a Shelf image; `notify`: announce what is found.
    fn qr_scan(&mut self, path: &str, notify: bool) {
        if thumbs::is_image(path) {
            self.qr.insert(path.to_string(), None);
            qr::request(path, notify);
        }
    }

    fn on_qr(&mut self, path: String, codes: Vec<String>, notify: bool) {
        // only while the image is still on the Shelf (it may be gone by now)
        if !self.data.shelf.contains(&path) {
            return;
        }
        // the count only: what a code says can be a password and must never reach the log file
        if !codes.is_empty() {
            dbg_log(&format!("qr: {} code(s) in a Shelf image", codes.len()));
        }
        let found = !codes.is_empty();
        self.qr.insert(path.clone(), Some(codes));
        if notify && found {
            if self.hidden {
                self.qr_later = Some(path);
            } else {
                self.qr_notice(&path);
            }
        }
    }

    /// "QR code · link" with Open / Copy; tapping it shows the codes on the Shelf.
    fn qr_notice(&mut self, path: &str) {
        let Some(Some(codes)) = self.qr.get(path) else { return };
        let Some(first) = codes.first().cloned() else { return };
        let n = codes.len();
        let (what, sub) = qr_label(&first);
        let title = if n > 1 { format!("{n} QR codes · {what}") } else { format!("QR code · {what}") };
        self.notice(TIcon::G(ic::QR), &title, &sub, 10.0, feat(&self.gfx, Tab::Shelf), Tap::Qr);
        self.qr_toast = self.toast.as_ref().map(|t| (t.id, path.to_string()));
    }

    /// The first code of the QR notice with this toast id (None: it is another notice).
    fn qr_toast_code(&self, id: u64) -> Option<String> {
        let (tid, p) = self.qr_toast.as_ref()?;
        if *tid != id {
            return None;
        }
        self.qr.get(p)?.as_ref()?.first().cloned()
    }

    fn show_qr_page(&mut self, path: &str) {
        let Some(i) = self.data.shelf.iter().position(|p| p == path) else { return };
        self.expand(Some(Tab::Shelf));
        self.shelf_sel.clear();
        // back on the Shelf, that picture is in view
        self.shelf_scroll = i.min(self.shelf_max());
        self.qr_pop = Some(path.to_string());
        self.qr_copied = None;
    }

    /// A button on the QR notice: open (0) or copy (1) its first code.
    fn toast_qr(&mut self, a: u8) {
        let Some((id, p)) = self.qr_toast.take() else { return };
        if self.toast.as_ref().map(|t| t.id) != Some(id) {
            return;
        }
        self.toast = None;
        let Some(code) = self.qr.get(&p).and_then(|c| c.as_ref()?.first().cloned()) else { return };
        if self.qr_run(&code, a) {
            let what = match qr::kind(&code) {
                qr::Kind::Link(_) => "Link copied",
                qr::Kind::Wifi(_, Some(_)) => "Password copied",
                qr::Kind::Wifi(_, None) => "Network name copied",
                qr::Kind::Text(_) => "Text copied",
            };
            self.toast(TIcon::G(ic::CHECK), what, "", 1.4, GREEN, None);
        }
        self.kick();
    }

    /// Open (0) a link in the browser, or copy (1) what the code is for: the link, the Wi-Fi
    /// password (the network name when it has none), the text. Returns true when copied.
    fn qr_run(&mut self, code: &str, a: u8) -> bool {
        let kind = qr::kind(code);
        if a == 0 {
            if let qr::Kind::Link(u) = &kind {
                shelf::open(u);
            }
            return false;
        }
        let text = match kind {
            qr::Kind::Link(u) => u,
            qr::Kind::Wifi(_, Some(p)) => p,
            qr::Kind::Wifi(s, None) => s,
            qr::Kind::Text(t) => t,
        };
        // copied from a QR code, not by the user: it stays out of the Clipboard tab
        if clip::write(self.hwnd, &text) {
            self.clip_ignore = Some(text);
            true
        } else {
            false
        }
    }

    pub fn tray_cmd(&mut self, cmd: u32) {
        if cmd == tray::CMD_THEME {
            if let Some(p) = theme::ensure_file() {
                tray::edit_file(&p);
            }
        } else if cmd == tray::CMD_LINE {
            self.data.settings.line_mode = !self.data.settings.line_mode;
            self.dirty = true;
            self.kick();
        } else if cmd == tray::CMD_CLOSE_AUTO || cmd == tray::CMD_CLOSE_CLICK {
            self.data.settings.close_on_click = cmd == tray::CMD_CLOSE_CLICK;
            self.dirty = true;
            if self.expanded && !self.drag_expanded {
                system::watch_clicks(self.data.settings.close_on_click);
                self.collapse_at = None;
            }
        } else if cmd == tray::CMD_MINI_LYRICS {
            self.toggle_mini_lyrics();
        } else if cmd == tray::CMD_SETTINGS {
            self.user_hidden = false;
            self.apply_visibility();
            if !self.hidden {
                self.expand(Some(Tab::Settings));
            }
        } else if cmd == tray::CMD_SHOTS {
            self.data.settings.screenshots = !self.data.settings.screenshots;
            if self.data.settings.screenshots {
                // only screenshots taken from now on
                self.shots = shots::Watcher::new();
            }
            self.dirty = true;
        } else if cmd == tray::CMD_AUTOSTART {
            self.autostart = !self.autostart;
            tray::set_autostart(self.autostart);
        } else if cmd == tray::CMD_UPDATE {
            self.update_cmd();
        } else if cmd == tray::CMD_HIDE {
            if self.user_hidden {
                self.user_hidden = false;
                self.apply_visibility();
            } else {
                self.leave(Leave::Hide);
            }
        }
    }

    /// Hidden while the user asked for it or a full-screen app is in front.
    fn apply_visibility(&mut self) {
        if self.leaving.is_some() {
            return;
        }
        let want = self.user_hidden || system::fullscreen_busy(self.hidden);
        if want != self.hidden {
            dbg_log(&format!("visibility: hidden={want} (by user: {})", self.user_hidden));
            self.hidden = want;
            // a hidden window hears nothing more from the mouse, so what it knew is out of date
            self.forget_pointer();
            unsafe {
                if want {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                } else {
                    // to the very top: a game that made itself topmost went above the island
                    // while it was hidden, and may still be open
                    let _ = SetWindowPos(
                        self.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
            }
            if !want {
                if self.data.settings.line_mode {
                    // back as the line it was, not a pill left over from before or while hidden;
                    // the window appearing under a resting cursor is not a hover yet
                    self.no_hover_until = Some(Instant::now() + Duration::from_millis(350));
                    if self.target().3 == ViewKey::Line {
                        self.view = ViewKey::Line;
                        self.line_from_pill = false;
                    }
                }
                self.pop_in();
            }
        }
    }

    /// Drop everything known about the mouse, as if it had left long ago.
    fn forget_pointer(&mut self) {
        if self.press.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        self.mouse = None;
        self.tracking = false;
        self.drag_hover = false;
        self.hover_on = false;
        self.hover_in = None;
        self.hover_out = None;
        self.hov = 0.0;
        self.hov_v = 0.0;
        self.isl_px = (0, 0, 0, 0);
    }

    /// Tray click or a second launch: unhide and open the island.
    pub fn show_and_open(&mut self) {
        match self.leaving {
            Some((Leave::Quit, _, _)) => return,
            // changed their mind halfway through hiding
            Some((Leave::Hide, _, _)) => self.leaving = None,
            None => {}
        }
        self.user_hidden = false;
        self.apply_visibility();
        if !self.hidden {
            self.expand(None);
        }
    }
}

// ------------------------------------------------------------------ widgets

/// A feature's color: the one picked on the Colors page (theme.conf), else the built-in one.
fn feat(g: &Gfx, t: Tab) -> C {
    let i = TABS.iter().position(|x| *x == t).unwrap_or(0);
    if let Some(c) = g.theme.feature[i] {
        return c;
    }
    match t {
        Tab::Media => WHITE,
        Tab::Focus => FOCUS_C,
        Tab::Water => WATER_C,
        Tab::Shelf => SHELF_C,
        Tab::Tasks => TASK_C,
        Tab::Clipboard => CLIP_C,
        Tab::Settings => SETTINGS_C,
    }
}

/// The icon color of an open tab: a color picked for that feature wins, then theme.conf
/// `icon_active`, then the built-in color.
fn tab_color(g: &Gfx, t: Tab) -> C {
    let i = TABS.iter().position(|x| *x == t).unwrap_or(0);
    match g.theme.feature[i] {
        Some(c) => c,
        None => active_icon(g, feat(g, t)),
    }
}

fn draw_tab_icon(g: &Gfx, t: Tab, r: R, col: C) {
    draw_tab_icon_sized(g, t, r, col, 14.0);
}

/// A feature's icon at any size (the Colors page draws a small island).
fn draw_tab_icon_sized(g: &Gfx, t: Tab, r: R, col: C, size: f32) {
    match t {
        Tab::Media => g.icon(ic::MUSIC, r, size, col),
        Tab::Focus => g.icon(ic::TIMER, r, size, col),
        Tab::Water => {
            let (w, h) = (size * 11.0 / 14.0, size * 15.0 / 14.0);
            if let Some(geo) = g.drop_geo(R::new(r.cx() - w / 2.0, r.cy() - h / 2.0, w, h)) {
                g.fill_geo(&geo, g.legible(g.text_alpha(g.icon_color(col))));
            }
        }
        Tab::Shelf => g.icon(ic::FOLDER, r, size, col),
        Tab::Tasks => g.icon(ic::TASKS, r, size, col),
        Tab::Clipboard => g.icon(ic::CLIP, r, size, col),
        Tab::Settings => g.icon(ic::GEAR, r, size, col),
    }
}

/// A lively version of a color for tints and glows: dark colors are lifted so a navy
/// background still lights its tile blue (greys stay grey).
fn vivid(c: C) -> C {
    let (h, s, v) = theme::to_hsv(c);
    nudge(theme::hsv(h, s, v.max(0.72)))
}

/// The ring around a picked tile: its own color, or a light ring for dark and grey colors.
fn ring_color(c: C) -> C {
    let (h, s, v) = theme::to_hsv(c);
    if s < 0.15 {
        WHITE.a(0.55)
    } else {
        nudge(theme::hsv(h, s.min(0.75), v.max(0.8)))
    }
}

/// A color that is never exactly white (the renderer swaps pure white for the text color).
fn nudge(c: C) -> C {
    C(c.0.min(0.999), c.1, c.2, c.3)
}

/// Fills a rounded rectangle with background stops (one = solid; several = a gradient from
/// the bottom up), the way the island paints its background.
fn fill_stops(g: &Gfx, r: R, rad: f32, stops: &[(f32, C)]) {
    if stops.len() < 2 {
        g.rrect(r, rad, nudge(stops.first().map(|s| s.1).unwrap_or(BLACK)));
        return;
    }
    g.push_rr(r, rad, 1.0);
    let n = ((r.h / 2.0).ceil() as usize).max(8);
    for k in 0..n {
        let f = 1.0 - (k as f32 + 0.5) / n as f32;
        let c = at_stop(stops, f);
        g.rect(R::new(r.x, r.y + r.h * k as f32 / n as f32, r.w, r.h / n as f32 + 0.6), nudge(C(c.0, c.1, c.2, 1.0)));
    }
    g.pop();
}

/// Whether a key combination may become the mute shortcut. A global shortcut takes its keys
/// from every app, so: two modifiers at least (one Ctrl or Alt plus a letter is nearly always
/// some app's: copy, paste, save, menus), and none of the keys Windows itself or text editing
/// lives on. The error says why, with `{}` for the shortcut's name.
fn hotkey_allowed(mods: u32, vk: u32) -> Result<(), &'static str> {
    let (ctrl, alt, shift, win) = (mods & MOD_CONTROL.0 != 0, mods & MOD_ALT.0 != 0, mods & MOD_SHIFT.0 != 0, mods & MOD_WIN.0 != 0);
    if [ctrl, alt, shift, win].iter().filter(|b| **b).count() < 2 {
        return Err("Use two of Ctrl, Alt, Shift, Win");
    }
    let k = |v: VIRTUAL_KEY| v.0 as u32 == vk;
    // switching windows and tabs, Task Manager, typing and moving around in text
    let editing = [VK_TAB, VK_ESCAPE, VK_RETURN, VK_BACK, VK_DELETE, VK_INSERT, VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_HOME, VK_END, VK_PRIOR, VK_NEXT];
    if editing.iter().any(|v| k(*v)) {
        return Err("{} is used by Windows or for typing");
    }
    // Win+Shift+S: screen snip; Win+Ctrl+D / F4: new / close virtual desktop
    if win && ((shift && vk == 'S' as u32) || (ctrl && (vk == 'D' as u32 || k(VK_F4)))) {
        return Err("{} is used by Windows");
    }
    Ok(())
}

/// "Ctrl+Alt+M", "Ctrl+Shift+F9", "Win+Alt+Space": a shortcut as the Settings page shows it.
fn hotkey_name(mods: u32, vk: u32) -> String {
    let mut s = String::new();
    for (bit, name) in [(MOD_CONTROL.0, "Ctrl"), (MOD_ALT.0, "Alt"), (MOD_SHIFT.0, "Shift"), (MOD_WIN.0, "Win")] {
        if mods & bit != 0 {
            s.push_str(name);
            s.push('+');
        }
    }
    let key = match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => unsafe {
            // the keyboard's own name for it ("Space", "Page Up", "Num 5"); extended keys need
            // bit 24, or the arrows come out as the number pad's
            let sc = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            let ext = matches!(vk, 0x21..=0x2E | 0x5B..=0x5D | 0x6F | 0x90);
            let lp = ((sc as i32) << 16) | if ext { 1 << 24 } else { 0 };
            let mut buf = [0u16; 64];
            let n = GetKeyNameTextW(lp, &mut buf);
            if n > 0 {
                String::from_utf16_lossy(&buf[..n as usize])
            } else {
                format!("Key {vk}")
            }
        },
    };
    s + &key
}

/// A microphone with a line through it.
fn draw_mic_off(g: &Gfx, r: R, size: f32, col: C) {
    g.icon(ic::MIC, r, size, col);
    let h = size * 0.62;
    g.line(r.cx() - h, r.cy() - h, r.cx() + h, r.cy() + h, 1.6, g.ico(g.legible(g.text_alpha(col))));
}

/// A color from pasted text: #RRGGBB, RRGGBB, #RGB, or three numbers 0-255 (rgb(30, 79, 216)).
fn parse_color_text(s: &str) -> Option<C> {
    let s = s.trim();
    let nums: Vec<f32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.parse::<f32>().ok())
        .collect();
    let low = s.to_ascii_lowercase();
    if low.starts_with("rgb") || (nums.len() == 3 && s.contains(',')) {
        if nums.len() >= 3 && nums[..3].iter().all(|n| *n <= 255.0) {
            return theme::color(&format!("#{:02X}{:02X}{:02X}", nums[0] as u8, nums[1] as u8, nums[2] as u8));
        }
        return None;
    }
    let h = s.trim_start_matches('#');
    match h.len() {
        3 if h.chars().all(|c| c.is_ascii_hexdigit()) => theme::color(&h.chars().flat_map(|c| [c, c]).collect::<String>()),
        6 | 8 if h.chars().all(|c| c.is_ascii_hexdigit()) => theme::color(&h[..6]),
        _ => None,
    }
}

/// A gradient's color at `f` (0 bottom .. 1 top).
fn at_stop(stops: &[(f32, C)], f: f32) -> C {
    let Some(first) = stops.first() else { return BLACK };
    if f <= first.0 {
        return first.1;
    }
    for w in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (w[0], w[1]);
        if f <= p1 {
            return c0.mix(c1, if p1 > p0 { (f - p0) / (p1 - p0) } else { 1.0 });
        }
    }
    stops.last().map(|s| s.1).unwrap_or(BLACK)
}

/// A Settings card's background (lighter while the mouse is on a card that can be clicked).
fn card_base(g: &Gfx, r: R, hover: bool) {
    g.rrect(r, 10.0, if hover { surf(g, true) } else { surf(g, false) });
}

/// A Settings card's icon, name and short explanation, kept within `w` from the card's left.
fn card_text(g: &Gfx, r: R, glyph: char, title: &str, sub: &str, w: f32) {
    g.icon(glyph, R::new(r.x + 10.0, r.y, 20.0, r.h), 14.0, btn_icon(g, WHITE.a(0.85)));
    let tw = (w - 42.0).max(20.0);
    g.text(title, R::new(r.x + 40.0, r.y + 6.0, tw, 18.0), 12.5, 600, WHITE, Al::L);
    g.text(sub, R::new(r.x + 40.0, r.y + 23.0, tw, 16.0), 10.5, 500, T3, Al::L);
}

/// A Settings card with a switch: click anywhere on it. `on: None` greys it out (nothing to
/// switch). Shows "On" / "Off" beside the switch, as Windows does.
#[allow(clippy::too_many_arguments)]
fn setting_card(g: &Gfx, ui: &mut Ui, r: R, glyph: char, title: &str, sub: &str, on: Option<bool>, opt: Opt, col: C) {
    let hv = on.is_some() && ui.hit(r, Act::Opt(opt));
    card_base(g, r, hv);
    card_text(g, r, glyph, title, sub, r.w - 96.0);
    let state = on.unwrap_or(false);
    let track = R::new(r.right() - 50.0, r.cy() - 9.0, 38.0, 18.0);
    let word = if state { "On" } else { "Off" };
    g.text(word, R::new(track.x - 34.0, r.y, 28.0, r.h), 11.5, 500, if on.is_some() { T2 } else { T3 }, Al::R);
    if state {
        g.rrect(track, 9.0, accent_btn(g, col));
        g.circle(track.right() - 9.0, track.cy(), 5.5, accent_fg(g));
    } else {
        g.rrect_stroke(track.inset(0.75, 0.75), 8.25, WHITE.a(if on.is_some() { 0.6 } else { 0.25 }), 1.5, false);
        g.circle(track.x + 9.0, track.cy(), 4.5, WHITE.a(if on.is_some() { 0.8 } else { 0.3 }));
    }
}

fn draw_battery(g: &Gfx, x: f32, cy: f32, w: f32, h: f32, f: f32, charging: bool) {
    let body = R::new(x, cy - h / 2.0, w, h);
    g.rrect_stroke(body, 3.0, g.ico(WHITE.a(0.55)), 1.2, false);
    g.rrect(R::new(body.right() + 1.0, cy - h * 0.2, 2.0, h * 0.4), 1.0, g.ico(WHITE.a(0.55)));
    let col = if charging {
        GREEN
    } else if f <= 0.2 {
        RED
    } else {
        WHITE
    };
    let inner = body.inset(2.2, 2.2);
    g.rrect(R::new(inner.x, inner.y, (inner.w * f.clamp(0.04, 1.0)).max(1.5), inner.h), 1.5, g.ico(col));
}

/// Background of a regular button (theme.conf `button` / `button_hover`).
fn surf(g: &Gfx, hover: bool) -> C {
    let t = &g.theme;
    match (hover, t.button, t.button_hover) {
        (true, _, Some(h)) => h,
        (true, Some(b), None) => b.mix(g.neutral(), 0.18),
        (true, None, None) => SURF_H,
        (false, Some(b), _) => b,
        (false, None, _) => SURF,
    }
}

/// Background of the selected button (theme.conf `button_active`); hovering never changes it.
fn surf_active(g: &Gfx) -> C {
    let t = &g.theme;
    match (t.button_active, t.button) {
        (Some(a), _) => a,
        (None, Some(b)) => b.mix(g.neutral(), 0.18),
        (None, None) => SURF_H,
    }
}

/// Text color on a regular button (theme.conf `button_text`), keeping `c`'s transparency.
fn btn_fg(g: &Gfx, c: C) -> C {
    match g.theme.button_text {
        Some(t) => g.text_alpha(C(t.0, t.1, t.2, t.3 * c.3)),
        None => c,
    }
}

/// Icon color on a regular button: theme.conf `icon` wins (applied when drawn), else `button_text`.
fn btn_icon(g: &Gfx, c: C) -> C {
    if g.theme.icon.is_some() {
        c
    } else {
        btn_fg(g, c)
    }
}

/// Icon of the open tab / a switched-on toggle: the feature's color unless theme.conf sets `icon_active`.
fn active_icon(g: &Gfx, feature: C) -> C {
    match g.theme.icon_active {
        Some(a) => C(a.0, a.1, a.2, a.3 * feature.3),
        None => feature,
    }
}

/// Fill of a main button: the feature's color unless theme.conf sets `button_accent`.
fn accent_btn(g: &Gfx, feature: C) -> C {
    g.theme.button_accent.unwrap_or(feature)
}

/// Text on a main button: theme.conf `button_accent_text`, else black, or white when a custom
/// `button_accent` is dark.
fn accent_fg(g: &Gfx) -> C {
    if let Some(t) = g.theme.button_accent_text {
        return t;
    }
    match g.theme.button_accent {
        Some(a) if crate::theme::luma(std::iter::once(a)) < 0.5 => C(0.9999, 0.9999, 0.9999, 1.0),
        _ => BLACK,
    }
}

fn icon_btn(g: &Gfx, ui: &mut Ui, r: R, ch: char, size: f32, col: C, act: Act, bg: bool) {
    let hv = ui.hit(r, act);
    if hv || bg {
        g.rrect(r, (r.h / 2.0).min(11.0), surf(g, hv));
    }
    g.icon(ch, r, size, btn_icon(g, col));
}

fn pill_btn(g: &Gfx, ui: &mut Ui, r: R, text: &str, fg: C, bg: C, act: Act) {
    let hv = ui.hit(r, act);
    let bg = if hv { bg.mix(WHITE, 0.15) } else { bg };
    g.rrect(r, r.h / 2.0, bg);
    g.text(text, r, 12.5, 600, fg, Al::C);
}

/// "Label  [−] value [+]" — returns total width.
#[allow(clippy::too_many_arguments)]
fn stepper(g: &Gfx, ui: &mut Ui, x: f32, y: f32, label: &str, val: &str, minus: Act, plus: Act, field: Field, edit: Option<(&str, bool)>) -> f32 {
    let lw = g.measure(label, 11.5, 500) + 8.0;
    g.text(label, R::new(x, y, lw, 26.0), 11.5, 500, T3, Al::L);
    let bx = x + lw;
    let box_w = 108.0;
    let b = R::new(bx, y, box_w, 26.0);
    g.rrect(b, 9.0, surf(g, false));
    let mr = R::new(bx, y, 26.0, 26.0);
    let pr = R::new(bx + box_w - 26.0, y, 26.0, 26.0);
    let mh = ui.hit(mr, minus);
    let ph = ui.hit(pr, plus);
    if mh {
        g.rrect(mr, 9.0, surf(g, true));
    }
    if ph {
        g.rrect(pr, 9.0, surf(g, true));
    }
    g.icon(ic::MINUS, mr, 9.0, btn_icon(g, if mh { WHITE } else { T2 }));
    g.icon(ic::ADD, pr, 9.0, btn_icon(g, if ph { WHITE } else { T2 }));
    let vr = R::new(bx + 26.0, y, box_w - 52.0, 26.0);
    let vh = ui.hit(vr, Act::Edit(field));
    value_box(g, vr, val, edit, vh, 11.5);
    lw + box_w
}

/// A number shown in place; while being edited it becomes a small text box with a caret.
fn value_box(g: &Gfx, r: R, val: &str, edit: Option<(&str, bool)>, hover: bool, size: f32) {
    match edit {
        Some((buf, caret)) => {
            g.rrect(r.inset(0.0, 2.0), 7.0, WHITE.a(0.14));
            g.rrect_stroke(r.inset(0.5, 2.5), 6.5, WHITE.a(0.55), 1.0, false);
            let tw = g.measure(buf, size, 600);
            let tx = r.cx() - tw / 2.0;
            g.text(buf, R::new(tx, r.y, tw + 4.0, r.h), size, 600, WHITE, Al::L);
            if caret {
                g.line(tx + tw + 1.5, r.cy() - 6.0, tx + tw + 1.5, r.cy() + 6.0, 1.3, g.txt(WHITE));
            }
        }
        None => {
            if hover {
                g.rrect(r.inset(0.0, 2.0), 7.0, WHITE.a(0.08));
            }
            g.text(val, r, size, 600, btn_fg(g, WHITE), Al::C);
        }
    }
}

/// Whole number; for volumes, "2.5" / "2,5" / "2.5l" mean litres and "500ml" millilitres.
fn parse_amount(s: &str, volume: bool) -> Option<u32> {
    let s = s.replace(' ', "").replace(',', ".");
    if s.is_empty() {
        return None;
    }
    if volume {
        if let Some(n) = s.strip_suffix("ml") {
            return n.parse::<f32>().ok().map(|v| v.round() as u32);
        }
        let litres = s.ends_with('l') || (s.contains('.') && s.trim_end_matches('l').parse::<f32>().map(|v| v < 20.0).unwrap_or(false));
        let n = s.trim_end_matches('l').parse::<f32>().ok()?;
        return Some(if litres { (n * 1000.0).round() as u32 } else { n.round() as u32 });
    }
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Task time: "" clears it; "14:30", "14.30", "1430", "9" set a clock time (tomorrow if
/// already past); "30m", "2h", "1h30" are relative to now. None means "couldn't parse".
#[cfg(test)]
fn parse_due(s: &str, now: i64) -> Option<Option<i64>> {
    parse_due_on(s, now, now.div_euclid(86400))
}

/// Like parse_due, but a clock time lands on `day` when that is a later day than today.
fn parse_due_on(s: &str, now: i64, day: i64) -> Option<Option<i64>> {
    let s: String = s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return Some(None);
    }
    if s.contains('h') || s.contains('m') {
        let (mut total, mut num, mut seen_h) = (0i64, String::new(), false);
        for c in s.chars() {
            match c {
                '0'..='9' => num.push(c),
                'h' => {
                    total += num.parse::<i64>().ok()? * 3600;
                    num.clear();
                    seen_h = true;
                }
                'm' => {
                    total += num.parse::<i64>().ok()? * 60;
                    num.clear();
                }
                _ => return None,
            }
        }
        if !num.is_empty() {
            // "1h30" -> trailing minutes
            if !seen_h {
                return None;
            }
            total += num.parse::<i64>().ok()? * 60;
        }
        return (total > 0).then(|| Some((now + total) / 60 * 60));
    }
    let (h, m) = if let Some((a, b)) = s.split_once([':', '.']) {
        (a.parse::<i64>().ok()?, b.parse::<i64>().ok()?)
    } else {
        let n = s.parse::<i64>().ok()?;
        match s.len() {
            1 | 2 => (n, 0),
            3 | 4 => (n / 100, n % 100),
            _ => return None,
        }
    };
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    if day > now.div_euclid(86400) {
        return Some(Some(day * 86400 + h * 3600 + m * 60));
    }
    let day = now.div_euclid(86400) * 86400;
    let mut t = day + h * 3600 + m * 60;
    if t <= now {
        t += 86400;
    }
    Some(Some(t))
}

/// Which kind of file an extension is (0 PDF, 1 document, 2 spreadsheet, 3 slides, 4 image,
/// 5 archive, 6 audio, 7 video, 8 program, 9 anything else).
fn ext_kind(ext: &str) -> usize {
    match ext {
        "PDF" => 0,
        "DOC" | "DOCX" | "RTF" | "TXT" | "MD" => 1,
        "XLS" | "XLSX" | "CSV" => 2,
        "PPT" | "PPTX" | "KEY" => 3,
        "PNG" | "JPG" | "JPEG" | "GIF" | "WEBP" | "BMP" | "SVG" | "HEIC" => 4,
        "ZIP" | "RAR" | "7Z" | "TAR" | "GZ" => 5,
        "MP3" | "WAV" | "FLAC" | "M4A" | "OGG" => 6,
        "MP4" | "MOV" | "MKV" | "AVI" | "WEBM" => 7,
        "EXE" | "MSI" => 8,
        _ => 9,
    }
}

/// The color of a file type's badge. With a Shelf color picked (a ready-made look, or Icons ▸
/// Shelf on the Colors page) every type gets its own shade of that color, so the badges match
/// the theme and the types can still be told apart; otherwise the usual colors per type.
fn ext_color(g: &Gfx, ext: &str) -> C {
    let kind = ext_kind(ext);
    if let Some(base) = g.theme.feature[3] {
        // (strength, brightness) of each kind's shade, relative to the Shelf color
        const SHADES: [(f32, f32); 10] = [
            (1.0, 1.0),
            (0.7, 0.86),
            (1.0, 0.7),
            (0.5, 1.0),
            (0.85, 0.82),
            (0.45, 0.74),
            (1.0, 0.88),
            (0.65, 0.64),
            (0.3, 0.6),
            (0.22, 0.54),
        ];
        let (h, s, v) = theme::to_hsv(base);
        let (sm, vm) = SHADES[kind];
        return theme::hsv(h, (s * sm).clamp(0.0, 1.0), (v * vm).clamp(0.18, 0.999));
    }
    const ORIGINAL: [u32; 10] = [0xE5484D, 0x3E7BFA, 0x30A46C, 0xF76B15, 0x8E4EC6, 0xC9A227, 0xD6409F, 0x12A594, 0x6E6E73, 0x5B5B66];
    C::hex(ORIGINAL[kind])
}

/// Text on a colored badge: white, or near-black on a light badge.
fn on_color(c: C) -> C {
    if theme::luma(std::iter::once(c)) > 0.6 {
        C(0.1, 0.1, 0.11, 1.0)
    } else {
        PURE_WHITE
    }
}

/// "now", "5m", "2h", "3d": how long ago `secs` seconds were.
fn ago(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

fn clipboard_text() -> Option<String> {
    unsafe {
        OpenClipboard(HWND::default()).ok()?;
        let r = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0usize;
            while *p.add(n) != 0 && n < 4096 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(hg);
            Some(s)
        })();
        let _ = CloseClipboard();
        r
    }
}

pub fn dbg_log(s: &str) {
    if std::env::var_os("VEYRA_DEBUG").is_some() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("veyra.log")) {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000_000).unwrap_or(0);
            let _ = writeln!(f, "{ms:>9} {s}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hotkey_allowed, parse_color_text, hotkey_name, left_label, parse_amount, parse_due, parse_due_on, passed_alerts, reminder_due};
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

    #[test]
    fn pasted_colors() {
        let hex = |s: &str| parse_color_text(s).map(crate::theme::hex);
        assert_eq!(hex(" #1e4fd8 ").as_deref(), Some("#1E4FD8"));
        assert_eq!(hex("1E4FD8").as_deref(), Some("#1E4FD8"));
        assert_eq!(hex("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(hex("#1E4FD8CC").as_deref(), Some("#1E4FD8"));
        assert_eq!(hex("rgb(30, 79, 216)").as_deref(), Some("#1E4FD8"));
        assert_eq!(hex("30, 79, 216").as_deref(), Some("#1E4FD8"));
        assert_eq!(hex("hello"), None);
        assert_eq!(hex("rgb(300, 0, 0)"), None);
    }

    // 2026-11-03 20:00:00 in the naive-local epoch used by the app
    const NOW: i64 = 20760 * 86400 + 20 * 3600;
    const DAY: i64 = 20760 * 86400;

    #[test]
    fn clock_times() {
        assert_eq!(parse_due("21:30", NOW), Some(Some(DAY + 21 * 3600 + 30 * 60)));
        assert_eq!(parse_due("21.30", NOW), Some(Some(DAY + 21 * 3600 + 30 * 60)));
        assert_eq!(parse_due("2130", NOW), Some(Some(DAY + 21 * 3600 + 30 * 60)));
        // already past today -> tomorrow
        assert_eq!(parse_due("930", NOW), Some(Some(DAY + 86400 + 9 * 3600 + 30 * 60)));
        assert_eq!(parse_due("9", NOW), Some(Some(DAY + 86400 + 9 * 3600)));
        assert_eq!(parse_due("25:00", NOW), None);
        assert_eq!(parse_due("12:61", NOW), None);
    }

    #[test]
    fn relative_and_clear() {
        assert_eq!(parse_due("30m", NOW), Some(Some(NOW + 1800)));
        assert_eq!(parse_due("2h", NOW), Some(Some(NOW + 7200)));
        assert_eq!(parse_due("1h30", NOW), Some(Some(NOW + 5400)));
        assert_eq!(parse_due("1h 30m", NOW), Some(Some(NOW + 5400)));
        assert_eq!(parse_due("", NOW), Some(None));
        assert_eq!(parse_due("abc", NOW), None);
    }

    #[test]
    fn clock_time_on_a_later_day() {
        assert_eq!(parse_due_on("14:30", NOW, 20765), Some(Some(20765 * 86400 + 14 * 3600 + 30 * 60)));
        // relative times stay relative to now
        assert_eq!(parse_due_on("30m", NOW, 20765), Some(Some(NOW + 1800)));
    }

    #[test]
    fn reminders() {
        let due = NOW + 2 * 86400;
        // nothing yet two days ahead
        assert_eq!(reminder_due(due, NOW, 0), (None, 0));
        // one day before: the 1-day notice
        let (w, bits) = reminder_due(due, due - 86400, 0);
        assert_eq!(w, Some(0));
        // the same again does nothing
        assert_eq!(reminder_due(due, due - 86000, bits), (None, bits));
        // 6 h, 3 h, 1 h, 30 min
        let (w, bits) = reminder_due(due, due - 6 * 3600, bits);
        assert_eq!(w, Some(1));
        let (w, bits) = reminder_due(due, due - 3 * 3600, bits);
        assert_eq!(w, Some(2));
        let (w, bits) = reminder_due(due, due - 3600, bits);
        assert_eq!(w, Some(3));
        let (w, bits) = reminder_due(due, due - 1800, bits);
        assert_eq!(w, Some(4));
        assert_eq!(bits, 0b11111);
        // asleep (or in a game) from 5 h to 40 min before: only the closest one, once
        let (w, bits) = reminder_due(due, due - 40 * 60, 0b1);
        assert_eq!(w, Some(3));
        assert_eq!(bits, 0b1111);
        // overdue: none of them (the due-now notice takes over)
        assert_eq!(reminder_due(due, due + 5, 0), (None, 0b11111));
        // a task added 2 h before its deadline skips the 1-day, 6 h and 3 h ones
        assert_eq!(passed_alerts(Some(NOW + 7200), NOW), 0b111);
        assert_eq!(passed_alerts(None, NOW), 0);
        assert_eq!(left_label(86400), "in 1 day");
        assert_eq!(left_label(6 * 3600), "in 6 hours");
        assert_eq!(left_label(3 * 3600 - 1), "in 3 hours");
        assert_eq!(left_label(3600), "in 1 hour");
        assert_eq!(left_label(1800), "in 30 minutes");
        assert_eq!(left_label(40 * 60), "in 40 minutes");
    }

    #[test]
    fn shortcut_rules() {
        let ctrl = MOD_CONTROL.0;
        let (alt, shift, win) = (MOD_ALT.0, MOD_SHIFT.0, MOD_WIN.0);
        // everyday app shortcuts
        assert!(hotkey_allowed(ctrl, 'C' as u32).is_err());
        assert!(hotkey_allowed(ctrl, 'V' as u32).is_err());
        assert!(hotkey_allowed(alt, 'F' as u32).is_err());
        assert!(hotkey_allowed(alt, 0x73).is_err()); // Alt+F4
        // Windows and text editing, even with two modifiers
        assert!(hotkey_allowed(ctrl | shift, 0x1B).is_err()); // Task Manager
        assert!(hotkey_allowed(ctrl | shift, 0x09).is_err()); // previous tab
        assert!(hotkey_allowed(ctrl | shift, 0x25).is_err()); // select a word
        assert!(hotkey_allowed(win | shift, 'S' as u32).is_err());
        // fine
        assert!(hotkey_allowed(ctrl | alt, 'M' as u32).is_ok());
        assert!(hotkey_allowed(ctrl | shift, 0x78).is_ok()); // Ctrl+Shift+F9
        assert!(hotkey_allowed(win | alt, 'M' as u32).is_ok());
    }

    #[test]
    fn shortcut_names() {
        assert_eq!(hotkey_name(MOD_CONTROL.0 | MOD_ALT.0, 'M' as u32), "Ctrl+Alt+M");
        assert_eq!(hotkey_name(MOD_CONTROL.0 | MOD_SHIFT.0, 0x78), "Ctrl+Shift+F9");
        assert_eq!(hotkey_name(MOD_WIN.0 | MOD_ALT.0, '5' as u32), "Alt+Win+5");
    }

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("330", false), Some(330));
        assert_eq!(parse_amount("25 min", false), Some(25));
        assert_eq!(parse_amount("2.5", true), Some(2500));
        assert_eq!(parse_amount("2,5", true), Some(2500));
        assert_eq!(parse_amount("1.5l", true), Some(1500));
        assert_eq!(parse_amount("750ml", true), Some(750));
        assert_eq!(parse_amount("2000", true), Some(2000));
        assert_eq!(parse_amount("", true), None);
    }
}
