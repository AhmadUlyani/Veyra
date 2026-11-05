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
use crate::{shelf, sound, system, tray};

// ------------------------------------------------------------------ style

const T2: C = C(1.0, 1.0, 1.0, 0.62);
const T3: C = C(1.0, 1.0, 1.0, 0.38);
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
}

// ------------------------------------------------------------------ types

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Media,
    Focus,
    Water,
    Shelf,
    Tasks,
}
const TABS: [Tab; 5] = [Tab::Media, Tab::Focus, Tab::Water, Tab::Shelf, Tab::Tasks];

#[derive(Clone, Copy, PartialEq, Debug)]
enum ViewKey {
    Compact(Live),
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
    Pomo,
    Task,
}

// compact-island metrics (DIP)
const PILL_H: f32 = 32.0;
const TASK_SOON_MIN: i64 = 15;
/// Clock in the closed island: bold, sized so the digits sit about as tall as the artwork.
const CLOCK_SIZE: f32 = 18.5;
/// Width of the closed island when nothing is going on: just an empty black notch.
const IDLE_W: f32 = 110.0;
// hover on the closed island: how much it grows, and the intent delays that keep a cursor just
// passing by (on its way to a browser tab) from waking it, and stop flicker at the edge
const HOVER_W: f32 = 16.0;
const HOVER_H: f32 = 4.0;
const HOVER_IN: Duration = Duration::from_millis(60);
const HOVER_OUT: Duration = Duration::from_millis(120);
/// critically damped (no overshoot): settles in ~180 ms growing, ~250 ms shrinking
const HOVER_W_IN: f32 = 26.0;
const HOVER_W_OUT: f32 = 18.0;
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
    Collapse,
    Tab(Tab),
    PlayPause,
    Prev,
    Next,
    Seek(R),
    Lyrics,
    LyricLine(f64),
    FocusToggle,
    FocusReset,
    FocusSkip,
    FocusLen(bool, i32),
    WaterAdd(u32),
    WaterUndo,
    WaterSet(u8, i32),
    ShelfItem(usize),
    ShelfRemove(usize),
    ShelfClear,
    ShelfAll,
    ShelfCheck(usize),
    TaskField,
    TaskTime(i64),
    Edit(Field),
    TaskAdd,
    TaskToggle(u64),
    TaskDel(u64),
    Toast,
    VolBar(R),
}

pub enum Deferred {
    DragOut(Vec<String>),
    Menu,
}

enum TIcon {
    G(char),
    Txt(String),
    Battery(f32, bool),
    Drop,
}

const K_INFO: u8 = 0;
const K_VOL: u8 = 1;
const K_BRIGHT: u8 = 2;

struct Toast {
    id: u64,
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
        let c = 2.0 * k.sqrt() * 0.74;
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
}

const SLIDE_SECS: f32 = 0.34;

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
    shelf_scroll: usize,
    drag_hover: bool,
    drag_expanded: bool,
    collapse_at: Option<Instant>,
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
        let gfx = Gfx::new(scale)?;
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
            shelf_scroll: 0,
            drag_hover: false,
            drag_expanded: false,
            collapse_at: None,
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
        };
        Ok(app)
    }

    pub fn start(&mut self) {
        self.set_tick(250);
        if let Ok(v) = std::env::var("VEYRA_DEMO") {
            self.demo_view(&v);
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

    /// Screenshot/testing aid: VEYRA_DEMO=media|lyrics|focus|water|shelf|tasks|toast|volume|caps|capsoff
    fn demo_view(&mut self, v: &str) {
        self.demo = true;
        match v {
            "lyrics" => {
                self.lyrics_open = true;
                self.expand(Some(Tab::Media));
                self.ensure_lyrics();
            }
            "media" => self.expand(Some(Tab::Media)),
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
            "slide" => {
                self.expand(Some(Tab::Media));
                self.demo_switch = Some(Instant::now() + Duration::from_millis(1500));
            }
            "tasks" => {
                if self.data.tasks.is_empty() {
                    let n = now().secs;
                    self.data.tasks.push(Task { id: 1, text: "Send the weekly report".into(), due: Some(n + 3600), ..Default::default() });
                    self.data.tasks.push(Task { id: 2, text: "Call the dentist".into(), due: Some(n - 600), ..Default::default() });
                    self.data.tasks.push(Task { id: 3, text: "Buy oat milk".into(), done: true, ..Default::default() });
                }
                self.input = "Review pull request".into();
                self.input_focus = true;
                self.adjust_due(0);
                self.expand(Some(Tab::Tasks));
            }
            "toast" => self.toast(TIcon::Drop, "Time to drink some water", "750 / 2000 ml today · tap to log", 600.0, WATER_C, Some(Tab::Water)),
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

    fn pomo_color(&self) -> C {
        if self.pomo.mode == PMode::Break {
            BREAK_C
        } else {
            FOCUS_C
        }
    }

    // -------------------------------------------------------------- toasts

    fn toast(&mut self, icon: TIcon, title: &str, sub: &str, secs: f32, accent: C, tab: Option<Tab>) {
        self.toast_seq += 1;
        self.toast = Some(Toast {
            id: self.toast_seq,
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
        self.expanded = true;
        self.toast = None;
        self.collapse_at = None;
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
        }
        if self.tab == Tab::Media && self.lyrics_open {
            self.ensure_lyrics();
        }
        self.kick();
    }

    fn collapse(&mut self) {
        self.commit_edit();
        self.shelf_sel.clear();
        self.expanded = false;
        self.input_focus = false;
        self.drag_expanded = false;
        self.collapse_at = None;
        self.press = None;
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
            };
            return (w, h, 30.0, ViewKey::Expanded(self.tab, lyr));
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
                let w = (TWO_PAD + TWO_ICON + TWO_GAP + text + TWO_PAD + 8.0).clamp(150.0, TWO_MAX_W);
                return (w, TWO_H, 20.0, ViewKey::Toast(t.id));
            }
        }
        let (live, sec) = self.lives();
        // hover growth is added on top in render(), driven by its own spring
        let w = self.compact_width(live) + self.segment_width(sec);
        (w, PILL_H, 13.0, ViewKey::Compact(live))
    }

    /// (primary activity, optional secondary activity shown at the right end of the island)
    fn lives(&self) -> (Live, Option<Live>) {
        let mut v = Vec::with_capacity(3);
        if self.media.is_some() {
            v.push(Live::Media);
        }
        if self.pomo.active() {
            v.push(Live::Pomo);
        }
        if self.task_soon().is_some() {
            v.push(Live::Task);
        }
        (v.first().copied().unwrap_or(Live::Idle), v.get(1).copied())
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
            // art | clock | equalizer — must mirror draw_compact
            Live::Media => 7.0 + (PILL_H - 10.0) + 6.0 + g.measure("88:88", CLOCK_SIZE, 700) + 24.0 + 6.0 + 18.0 + 14.0,
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
        let (tw, th, tr, key) = self.target();
        let hover_still = self.step_hover(n, dt, key);
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
                    });
                }
                _ => {
                    self.slide = None;
                    self.view_age = 0.0;
                }
            }
            self.view = key;
        }
        if let Some(sl) = &mut self.slide {
            sl.t += dt / SLIDE_SECS;
            if sl.t >= 1.0 {
                self.slide = None;
            }
        }
        self.sw.t = tw;
        self.sh.t = th;
        self.sr.t = tr;
        let mut rem = dt;
        while rem > 0.0 {
            let s = rem.min(1.0 / 240.0);
            self.sw.step(s);
            self.sh.step(s);
            self.sr.step(s);
            rem -= s;
        }
        self.view_age += dt;
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
            && hover_still;
        if settled {
            self.sw.snap();
            self.sh.snap();
            self.sr.snap();
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
        let compact = matches!(key, ViewKey::Compact(_)) && self.leaving.is_none() && !self.hidden;
        let raw = compact && (self.mouse.is_some() || self.drag_hover);
        let mut pending = false;
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
            let since = self.hover_out.map(|t| n.duration_since(t)).unwrap_or(HOVER_OUT);
            if since >= HOVER_OUT {
                self.hover_on = false;
            } else {
                pending = true;
            }
        }
        let t = if self.hover_on { 1.0 } else { 0.0 };
        let w = if self.hover_on { HOVER_W_IN } else { HOVER_W_OUT };
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
        let s = self.gfx.scale;
        let w = (self.sw.x + HOVER_W * self.hov).max(1.0);
        let h = (self.sh.x + HOVER_H * self.hov).max(0.0);
        // extra room on both sides for the concave "shoulders" where the notch meets the screen edge
        let wpx = ((w + 2.0 * SHOULDER) * s).ceil() as i32 + 2;
        let hpx = (h * s).ceil() as i32 + 1;
        let left = self.mon_cx - wpx / 2;
        let top = self.mon_top;
        if !self.gfx.begin(wpx, hpx) {
            return;
        }
        let island = R::new((wpx as f32 / s - w) / 2.0, 0.0, w, h);
        let mut ui = Ui { hits: Vec::with_capacity(32), mouse: self.mouse };
        self.draw(&self.gfx, &mut ui, island);
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
        g.rrect(shape, rad, BLACK);
        let f = SHOULDER.min(isl.h * 0.5);
        g.shoulder(isl.x, isl.y, f, true, BLACK);
        g.shoulder(isl.right(), isl.y, f, false, BLACK);
        let mut alpha = smooth((self.view_age - 0.05) / 0.22);
        if let Some((_, since, _)) = self.leaving {
            // content goes first, then the black shape follows it into the edge
            alpha *= 1.0 - smooth(since.elapsed().as_secs_f32() / 0.12);
        }
        if alpha <= 0.01 {
            return;
        }
        let (tw, th, _, _) = self.target();
        let c = R::new(isl.cx() - tw / 2.0, isl.y, tw, th);
        g.push_rr(shape, rad, alpha);
        match self.view {
            ViewKey::Compact(live) => self.draw_compact(g, ui, c, live),
            ViewKey::Toast(_) => {
                if let Some(t) = &self.toast {
                    self.draw_toast(g, ui, c, t);
                }
            }
            ViewKey::Expanded(tab, lyr) => self.draw_expanded(g, ui, c, tab, lyr),
        }
        g.pop();
    }

    // -------------------------------------------------------------- compact

    fn draw_art(&self, g: &Gfx, r: R, rad: f32) {
        match self.cur_art() {
            Some(b) => g.bitmap(b, r, rad),
            None => {
                g.rrect(r, rad, SURF_H);
                g.icon(ic::MUSIC, r, (r.h * 0.45).max(9.0), T2);
            }
        }
    }

    /// Thin progress line hugging the bottom edge (clipped by the rounded corners).
    fn bottom_progress(&self, g: &Gfx, c: R, f: f32, col: C) {
        if f > 0.0 {
            g.rect(R::new(c.x, c.bottom() - 2.0, c.w * f.clamp(0.0, 1.0), 2.0), col);
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
                let col = if now_due { RED } else { TASK_C };
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
            // idle: an empty black notch; hovering fades in a small clock so it reads as clickable
            Live::Idle => {
                let a = smooth((self.hov - 0.25) / 0.75);
                if a > 0.01 {
                    let r = R::new(c.x, c.y + 1.0 - 2.0 * (1.0 - a), c.w, c.h);
                    g.text(&hm(now().secs), r, 13.0, 600, WHITE.a(0.78 * a), Al::C);
                }
            }
            Live::Media => {
                let Some(m) = &self.media else { return };
                let acc = self.accent();
                let s = c.h - 10.0;
                self.draw_art(g, R::new(c.x + 7.0, c.y + 5.0, s, s), 6.0);
                let eq_x = c.right() - 14.0 - 18.0;
                self.draw_eq(g, eq_x, c.cy(), m.playing, acc);
                // clock in the middle, between the artwork and the equalizer
                let area = R::new(c.x + 7.0 + s + 6.0, c.y, (eq_x - 6.0) - (c.x + 7.0 + s + 6.0), c.h);
                g.text(&hm(now().secs), area, CLOCK_SIZE, 700, WHITE, Al::C);
                if m.dur > 0.0 {
                    self.bottom_progress(g, full, (self.media_pos() / m.dur) as f32, acc.a(0.85));
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
                self.bottom_progress(g, full, 1.0 - (rem / self.pomo.total) as f32, col.a(0.85));
            }
            Live::Task => {
                let Some((text, due, now_due)) = self.task_line() else { return };
                let col = if now_due { RED } else { TASK_C };
                g.icon(ic::TASKS, R::new(c.x + 10.0, c.y, 20.0, c.h), 13.0, col);
                let dw = g.measure(&due, 12.0, 600);
                g.text(&due, R::new(c.right() - 14.0 - dw, c.y, dw + 2.0, c.h), 12.0, 600, col, Al::R);
                g.text(&text, R::new(c.x + 36.0, c.y, c.w - 36.0 - dw - 24.0, c.h), 12.5, 500, WHITE.a(0.9), Al::L);
            }
        }
    }

    // -------------------------------------------------------------- toast

    fn draw_ticon(&self, g: &Gfx, icon: &TIcon, r: R, acc: C) {
        match icon {
            TIcon::G(ch) => g.icon(*ch, r, 17.0, acc),
            TIcon::Txt(s) => g.text(s, r, 16.0, 700, acc, Al::C),
            TIcon::Battery(f, ch) => draw_battery(g, r.cx() - 12.0, r.cy(), 22.0, 12.0, *f, *ch),
            TIcon::Drop => {
                if let Some(geo) = g.drop_geo(R::new(r.cx() - 7.0, r.cy() - 9.5, 14.0, 19.0)) {
                    g.fill_geo(&geo, acc);
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
            g.rrect(R::new(bar.x, bar.y, (bar.w * lv.clamp(0.0, 1.0)).max(6.0), bar.h), 3.0, t.accent);
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
            TIcon::Txt(s) => g.text(s, ir, 12.5, 700, t.accent, Al::C),
            TIcon::Battery(f, ch) => draw_battery(g, ir.cx() - 10.0, ir.cy(), 18.0, 10.0, *f, *ch),
            TIcon::Drop => {
                if let Some(geo) = g.drop_geo(R::new(ir.cx() - 5.5, ir.cy() - 7.5, 11.0, 15.0)) {
                    g.fill_geo(&geo, t.accent);
                }
            }
        }
        let tx = ir.right() + TWO_GAP;
        let tw = c.right() - tx - TWO_PAD;
        let mid = c.cy();
        g.text(&t.title, R::new(tx, mid - 17.0, tw, 18.0), TWO_TITLE, 600, WHITE, Al::L);
        g.text(&t.sub, R::new(tx, mid + 1.0, tw, 16.0), TWO_SUB, 400, T2, Al::L);
    }

    // -------------------------------------------------------------- expanded

    fn draw_expanded(&self, g: &Gfx, ui: &mut Ui, c: R, tab: Tab, lyr: bool) {
        let inner = R::new(c.x + 18.0, c.y + 12.0, c.w - 36.0, c.h - 26.0);
        ui.hit(R::new(c.x, c.y, c.w, 40.0), Act::Collapse);
        // tab bar
        for (i, t) in TABS.iter().enumerate() {
            let b = R::new(inner.x + i as f32 * 38.0, inner.y, 34.0, 28.0);
            let hv = ui.hit(b, Act::Tab(*t));
            let active = *t == tab;
            if active || hv {
                g.rrect(b, 10.0, if active { SURF_H } else { SURF });
            }
            let col = if active { tab_color(*t) } else if hv { WHITE } else { T3 };
            draw_tab_icon(g, *t, b, col);
        }
        // right: battery, date, clock
        let n = now();
        let mut x = inner.right();
        let clock = hm(n.secs);
        let cw = g.measure(&clock, 13.0, 600);
        g.text(&clock, R::new(x - cw - 2.0, inner.y, cw + 2.0, 28.0), 13.0, 600, WHITE, Al::R);
        x -= cw + 10.0;
        let date = n.date_label();
        let dw = g.measure(&date, 12.0, 500);
        g.text(&date, R::new(x - dw - 2.0, inner.y, dw + 2.0, 28.0), 12.0, 500, T2, Al::R);
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
                let p = 1.0 - (1.0 - sl.t.clamp(0.0, 1.0)).powi(3);
                // pages sit side by side (carousel), so they never overlap
                let dist = (c.w + sl.from_w) / 2.0 + 24.0;
                let mut dummy = Ui { hits: Vec::new(), mouse: None };
                let old_c = R::new(c.cx() - sl.from_w / 2.0, c.y, sl.from_w, sl.from_h);
                let all = R::new(-1e4, -1e4, 2e4, 2e4);
                g.translate(-sl.dir * dist * p, 0.0);
                g.push_rr(all, 0.0, (1.0 - p * 1.2).max(0.0));
                self.draw_body(g, &mut dummy, old_c, sl.from_tab, sl.from_lyr);
                g.pop();
                g.translate(sl.dir * dist * (1.0 - p), 0.0);
                g.push_rr(all, 0.0, (0.25 + p).min(1.0));
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
            g.rrect(lb, 10.0, if self.lyrics_open { acc.a(0.22) } else { SURF });
        }
        g.icon(ic::LYRICS, lb, 14.0, if self.lyrics_open { acc } else { T2 });
        let tw = col.w - 40.0;
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
            g.rrect(R::new(bar.x, bar.y, (bar.w * f).max(bar.h), bar.h), bar.h / 2.0, if hv { acc } else { acc.a(0.9) });
        }
        if hv {
            if let Some((mx, _)) = ui.mouse {
                let p = ((mx - bar.x) / bar.w).clamp(0.0, 1.0) as f64 * m.dur;
                g.text(&mmss(p), R::new(mx - 30.0, by - 22.0, 60.0, 14.0), 10.5, 600, WHITE, Al::C);
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
                        acc.a(0.9)
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
        g.rrect(b, 17.0, if hv { col } else { col.a(0.85) });
        g.text(btxt, b, 13.0, 600, BLACK, Al::C);
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
            g.rect(R::new(dr.x - 2.0, dr.bottom() - fh, dr.w + 4.0, fh + 2.0), WATER_C.a(0.9));
            g.pop();
            g.stroke_geo(&geo, WATER_C.a(0.5), 1.5);
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
            g.rrect(R::new(bar.x, bar.y, (bar.w * f).max(6.0), 6.0), 3.0, WATER_C);
        }
        let note = match self.next_water_in() {
            None => "Daily goal reached — nice work!".to_string(),
            Some(m) => format!("Next reminder in {m} min · {} drinks today", w.log.len()),
        };
        g.text(&note, R::new(x, c.y + 44.0, cw, 18.0), 11.5, 400, T3, Al::L);

        let by = c.y + 68.0;
        let b1 = R::new(x, by, 86.0, 32.0);
        pill_btn(g, ui, b1, "+250 ml", WHITE, SURF, Act::WaterAdd(250));
        let b2 = R::new(x + 94.0, by, 150.0, 32.0);
        pill_btn(g, ui, b2, &format!("+ Bottle · {} ml", w.bottle_ml), BLACK, WATER_C, Act::WaterAdd(w.bottle_ml));
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
                g.rrect_stroke(cr.inset(0.5, 0.5), 15.5, WATER_C.a(0.7), 1.2, false);
                let txt = format!("{buf} ml");
                let tw = g.measure(&txt, 12.5, 600);
                let tx = cr.cx() - tw / 2.0;
                g.text(&txt, R::new(tx, cr.y, tw + 4.0, cr.h), 12.5, 600, WHITE, Al::L);
                if caret {
                    let bw = g.measure(buf, 12.5, 600);
                    g.line(tx + bw + 1.0, cr.cy() - 7.0, tx + bw + 1.0, cr.cy() + 7.0, 1.3, WATER_C);
                }
            }
            None => {
                g.rrect(cr, 16.0, if ch { SURF_H } else { SURF });
                g.text("Custom", cr, 12.5, 600, T2, Al::C);
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
        let files = &self.data.shelf;
        if files.is_empty() {
            let r = c.inset(2.0, 2.0);
            g.rrect(r, 16.0, if self.drag_hover { SHELF_C.a(0.12) } else { WHITE.a(0.03) });
            g.rrect_stroke(r, 16.0, if self.drag_hover { SHELF_C } else { WHITE.a(0.22) }, 1.5, true);
            g.icon(ic::FOLDER, R::new(r.x, r.y + 18.0, r.w, 34.0), 24.0, if self.drag_hover { SHELF_C } else { T2 });
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
        let (tw, th, gap) = (80.0, 96.0, 9.0);
        let per = ((c.w + gap) / (tw + gap)).floor() as usize;
        let start = self.shelf_scroll.min(files.len().saturating_sub(per));
        for (k, path) in files.iter().enumerate().skip(start).take(per) {
            let i = k - start;
            let t = R::new(c.x + i as f32 * (tw + gap), c.y, tw, th);
            let hv = ui.hit(t, Act::ShelfItem(k));
            let sel = self.shelf_sel.contains(path);
            if sel {
                g.rrect(t, 14.0, if hv { SHELF_C.a(0.28) } else { SHELF_C.a(0.16) });
                g.rrect_stroke(t.inset(0.75, 0.75), 13.5, SHELF_C.a(0.85), 1.5, false);
            } else {
                g.rrect(t, 14.0, if hv { SURF_H } else { SURF });
            }
            let name = std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or(path.clone());
            let is_dir = std::path::Path::new(path).is_dir();
            let ext = std::path::Path::new(path)
                .extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            let badge = R::new(t.cx() - 17.0, t.y + 10.0, 34.0, 40.0);
            if is_dir {
                g.icon(ic::FOLDER, badge, 28.0, SHELF_C);
            } else {
                let bc = ext_color(&ext);
                g.rrect(badge, 7.0, bc.a(0.9));
                g.rrect(R::new(badge.right() - 10.0, badge.y, 10.0, 10.0), 3.0, WHITE.a(0.35));
                let e: String = ext.chars().take(4).collect();
                if e.is_empty() {
                    g.icon(ic::DOC, badge, 16.0, WHITE);
                } else {
                    g.text(&e, R::new(badge.x, badge.y + 14.0, badge.w, 20.0), 10.0, 700, WHITE, Al::C);
                }
            }
            g.text_wrap(&name, R::new(t.x + 6.0, t.y + 56.0, t.w - 12.0, 32.0), 11.0, 500, WHITE.a(0.85), Al::C);
            // selection circle: always shown in selection mode, otherwise on hover
            if hv || sel || !self.shelf_sel.is_empty() {
                let cr = R::new(t.x + 4.0, t.y + 4.0, 18.0, 18.0);
                let ch = ui.hit(cr, Act::ShelfCheck(k));
                if sel {
                    g.circle(cr.cx(), cr.cy(), 9.0, SHELF_C);
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
        g.text(&info, R::new(c.x, fy, c.w - 200.0, 20.0), 11.5, 400, if nsel > 0 { SHELF_C.mix(WHITE, 0.4) } else { T3 }, Al::L);
        // press and drag this to carry every file on the shelf at once
        let dr = R::new(c.right() - 74.0 - 8.0 - 100.0, fy - 2.0, 100.0, 24.0);
        let hv = ui.hit(dr, Act::ShelfAll);
        if nsel > 0 {
            g.rrect(dr, 12.0, if hv { SHELF_C.mix(WHITE, 0.2) } else { SHELF_C });
            g.text("Deselect", dr, 11.5, 700, BLACK, Al::C);
        } else {
            g.rrect(dr, 12.0, if hv { SHELF_C.a(0.35) } else { SHELF_C.a(0.18) });
            g.text(&format!("Drag all ({})", files.len()), dr, 11.5, 600, SHELF_C.mix(WHITE, 0.35), Al::C);
        }
        let cr = R::new(c.right() - 74.0, fy - 2.0, 74.0, 24.0);
        let hv = ui.hit(cr, Act::ShelfClear);
        g.rrect(cr, 12.0, if hv { RED.a(0.25) } else { SURF });
        g.text("Clear all", cr, 11.5, 600, if hv { RED } else { T2 }, Al::C);
        if self.drag_hover {
            g.rrect_stroke(c.inset(-4.0, -4.0), 16.0, SHELF_C, 1.5, true);
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
            Field::TaskTime => self.task_due.map(hm).unwrap_or_default(),
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
            Field::TaskTime => {
                if let Some(v) = parse_due(&t, now().secs) {
                    self.task_due = v;
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
                    Field::TaskTime => {}
                }
                self.dirty = true;
            }
        }
        self.kick();
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

    fn sorted_tasks(&self) -> Vec<&Task> {
        let mut v: Vec<&Task> = self.data.tasks.iter().collect();
        v.sort_by_key(|t| (t.done, t.due.is_none(), t.due.unwrap_or(0), t.id));
        v
    }

    fn draw_tasks(&self, g: &Gfx, ui: &mut Ui, c: R) {
        let n = now();
        // input row
        let field = R::new(c.x, c.y, c.w - 178.0, 32.0);
        let hv = ui.hit(field, Act::TaskField);
        g.rrect(field, 10.0, if self.input_focus { WHITE.a(0.12) } else if hv { SURF_H } else { SURF });
        if self.input_focus {
            g.rrect_stroke(field, 10.0, TASK_C.a(0.6), 1.2, false);
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
                g.line(cx, inner.y + 8.0, cx, inner.bottom() - 8.0, 1.5, TASK_C);
            }
            g.pop();
        }
        // time chip
        let tr = R::new(field.right() + 8.0, c.y, 128.0, 32.0);
        g.rrect(tr, 10.0, SURF);
        let mr = R::new(tr.x, tr.y, 30.0, 32.0);
        let pr = R::new(tr.right() - 30.0, tr.y, 30.0, 32.0);
        let mid = R::new(tr.x + 30.0, tr.y, tr.w - 60.0, 32.0);
        let mh = ui.hit(mr, Act::TaskTime(-900));
        let ph = ui.hit(pr, Act::TaskTime(900));
        let th = ui.hit(mid, Act::Edit(Field::TaskTime));
        g.icon(ic::MINUS, mr, 10.0, if mh { WHITE } else { T2 });
        g.icon(ic::ADD, pr, 10.0, if ph { WHITE } else { T2 });
        match (self.edit_of(Field::TaskTime), self.task_due) {
            (Some(e), _) => value_box(g, mid, "", Some(e), false, 12.5),
            (None, Some(d)) => g.text(&due_label(d, n.day()), mid, 12.5, 600, if th { WHITE } else { TASK_C }, Al::C),
            (None, None) => g.text("No time", mid, 12.0, 500, if th { WHITE } else { T3 }, Al::C),
        }
        let ar = R::new(c.right() - 34.0, c.y, 34.0, 32.0);
        let ah = ui.hit(ar, Act::TaskAdd);
        g.rrect(ar, 10.0, if ah { TASK_C } else { TASK_C.a(0.85) });
        g.icon(ic::ADD, ar, 13.0, BLACK);

        // list
        let tasks = self.sorted_tasks();
        let ly = c.y + 42.0;
        let rh = 30.0;
        let rows = ((c.bottom() - ly) / rh).floor().max(1.0) as usize;
        if tasks.is_empty() {
            g.text("No tasks yet", R::new(c.x, ly + 22.0, c.w, 22.0), 14.0, 600, T2, Al::C);
            g.text("Type a task, set a time (type 14:30 or 30m, or use − / +), press Enter.", R::new(c.x, ly + 44.0, c.w, 18.0), 11.5, 400, T3, Al::C);
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
            g.icon(if t.done { ic::BOX_ON } else { ic::BOX }, cb, 15.0, if t.done { TASK_C } else if ch { WHITE } else { T2 });
            let overdue = !t.done && t.due.map(|d| d <= n.secs).unwrap_or(false);
            let due_w = if t.due.is_some() { 86.0 } else { 0.0 };
            let txt = R::new(r.x + 36.0, r.y, r.w - 36.0 - due_w - 30.0, r.h);
            g.text(&t.text, txt, 13.0, 400, if t.done { T3 } else { WHITE }, Al::L);
            if t.done {
                let w = g.measure(&t.text, 13.0, 400).min(txt.w);
                g.line(txt.x, r.cy(), txt.x + w, r.cy(), 1.2, T3);
            }
            if let Some(d) = t.due {
                g.text(
                    &due_label(d, n.day()),
                    R::new(r.right() - due_w - 30.0, r.y, due_w, r.h),
                    12.0,
                    600,
                    if t.done { T3 } else if overdue { RED } else { TASK_C },
                    Al::R,
                );
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

    fn hit_at(&self, p: (f32, f32)) -> Option<Act> {
        self.hits.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, a)| *a)
    }

    fn act(&mut self, a: Act, mx: f32) {
        if let Some(e) = &self.edit {
            if a != Act::Edit(e.field) {
                self.commit_edit();
            }
        }
        if !matches!(a, Act::TaskField | Act::TaskTime(_) | Act::Edit(Field::TaskTime) | Act::TaskAdd) {
            self.input_focus = false;
        }
        match a {
            Act::Collapse => self.collapse(),
            Act::Tab(t) => {
                self.shelf_sel.clear();
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
            Act::Toast => {
                if let Some(t) = self.toast.take() {
                    if let Some(tab) = t.tab {
                        self.expand(Some(tab));
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

    fn adjust_due(&mut self, d: i64) {
        let n = now().secs;
        let round_up = |s: i64, step: i64| (s / step + 1) * step;
        let base = self.task_due.unwrap_or_else(|| round_up(n + 600, 900));
        let mut v = if self.task_due.is_some() { base + d } else { base };
        if v <= n {
            v = round_up(n, 300);
        }
        self.task_due = Some(v);
    }

    fn add_task(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            self.input_focus = true;
            return;
        }
        self.data.next_id += 1;
        self.data.tasks.push(Task { id: self.data.next_id, text, due: self.task_due, done: false, notified: false });
        self.input.clear();
        self.task_due = None;
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
                    self.toast(TIcon::G(ic::TIMER), "Focus session complete", &msg, 8.0, BREAK_C, Some(Tab::Focus));
                }
            }
            PMode::Break => {
                self.pomo.mode = PMode::Idle;
                if natural {
                    sound::chime();
                    self.toast(TIcon::G(ic::TIMER), "Break's over", "Ready for another focus session?", 8.0, FOCUS_C, Some(Tab::Focus));
                }
            }
            PMode::Idle => {}
        }
    }

    // -------------------------------------------------------------- periodic

    pub fn tick(&mut self) {
        let n = Instant::now();
        let mut redraw = false;

        if let Some(t) = &self.toast {
            if n >= t.until && !self.cursor_inside() {
                self.toast = None;
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
                crate::util::trim_memory();
                dbg_log("memory: trimmed");
            }
        }
        if self.pomo.active() && !self.pomo.paused && self.pomo.remaining() <= 0.0 {
            self.finish_phase(true);
            redraw = true;
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
        if self.mouse.is_none() && self.cursor_inside() {
            let mut p = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
            let s = self.gfx.scale;
            self.mouse = Some(((p.x - self.win.0) as f32 / s, (p.y - self.win.1) as f32 / s));
            self.tracking = false;
            redraw = true;
        }

        // auto-collapse when the island is open but the user went elsewhere
        if self.expanded && !self.demo {
            let fg = unsafe { GetForegroundWindow() };
            if self.cursor_inside() {
                self.collapse_at = None;
            } else if fg != self.hwnd && self.collapse_at.is_none() {
                self.collapse_at = Some(n + Duration::from_millis(if self.drag_expanded { 2500 } else { 1200 }));
            }
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
        self.set_tick(if fast { 90 } else { 250 });
        if (periodic || redraw) && !self.hidden {
            self.kick();
        }
    }

    fn per_second(&mut self) {
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
        if (7..23).contains(&t.h) && w.total() < w.goal_ml && self.water_mark.elapsed() >= iv {
            self.water_mark = Instant::now();
            let msg = format!("{} / {} ml today · tap to log", w.total(), w.goal_ml);
            sound::chime();
            self.toast(TIcon::Drop, "Time to drink some water", &msg, 10.0, WATER_C, Some(Tab::Water));
        }

        // task reminders
        let mut due: Vec<String> = Vec::new();
        for task in self.data.tasks.iter_mut() {
            if !task.done && !task.notified {
                if let Some(d) = task.due {
                    if d <= t.secs {
                        task.notified = true;
                        due.push(task.text.clone());
                    }
                }
            }
        }
        if !due.is_empty() {
            self.dirty = true;
            sound::chime();
            let sub = if due.len() == 1 { due[0].clone() } else { format!("{} and {} more", due[0], due.len() - 1) };
            self.toast(TIcon::G(ic::TASKS), "Task due now", &sub, 12.0, TASK_C, Some(Tab::Tasks));
        }

        // hide during full-screen apps / presentations
        self.apply_visibility();

        if self.dirty {
            self.save_now();
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
                    if new != old && self.lyrics_open {
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
                Event::DragEnter => {
                    self.drag_hover = true;
                    self.collapse_at = None;
                    if !self.expanded {
                        self.expanded = true;
                        self.drag_expanded = true;
                        self.toast = None;
                    }
                    self.tab = Tab::Shelf;
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
                            self.data.shelf.push(f);
                        }
                    }
                    self.dirty = true;
                    if self.drag_expanded {
                        self.collapse_at = Some(Instant::now() + Duration::from_millis(2500));
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

    pub fn on_mouse_move(&mut self, lp: LPARAM) {
        let p = self.local(lp);
        if self.mouse.is_none() {
            self.hover_in = Some(Instant::now());
        }
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
        self.mouse = Some(p);
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
                    if !paths.is_empty() {
                        self.shelf_sel.clear();
                        self.deferred = Some(Deferred::DragOut(paths));
                    }
                }
            }
        }
        self.kick();
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking = false;
        self.mouse = None;
        self.hover_out = Some(Instant::now());
        self.kick();
    }

    /// Buttons fire on press (snappy, and immune to the layout shifting under a
    /// still cursor). Shelf tiles wait for release so they can be dragged out.
    pub fn on_ldown(&mut self, lp: LPARAM) {
        let p = self.local(lp);
        self.mouse = Some(p);
        let hit = self.hit_at(p);
        if !self.expanded {
            match hit {
                Some(Act::VolBar(r)) => self.act(Act::VolBar(r), p.0),
                Some(Act::Tab(t)) => self.expand(Some(t)),
                Some(Act::Toast) => {
                    let tab = self.toast.as_ref().and_then(|t| t.tab);
                    self.toast = None;
                    self.expand(tab);
                }
                _ => self.expand(None),
            }
            return;
        }
        match hit {
            Some(a @ (Act::ShelfItem(_) | Act::ShelfAll)) => self.press = Some((Some(a), p.0, p.1)),
            Some(a) => self.act(a, p.0),
            None => {
                self.input_focus = false;
                self.commit_edit();
                self.kick();
            }
        }
    }

    pub fn on_lup(&mut self, lp: LPARAM) {
        let p = self.local(lp);
        // released without dragging: a click (open the file / toggle "select all")
        if let Some((Some(a @ (Act::ShelfItem(_) | Act::ShelfAll)), _, _)) = self.press.take() {
            if self.hit_at(p) == Some(a) {
                self.act(a, p.0);
            }
        }
    }

    pub fn on_rup(&mut self) {
        self.deferred = Some(Deferred::Menu);
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
            (Tab::Shelf, _) => {
                if steps < 0 {
                    self.shelf_scroll = (self.shelf_scroll + 1).min(self.data.shelf.len().saturating_sub(1));
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
            if self.expanded && !self.drag_expanded && !self.demo && !self.cursor_inside() {
                dbg_log("collapse: deactivate");
                self.collapse();
            }
        }
        self.kick();
    }

    pub fn on_key(&mut self, vk: u32) -> bool {
        if vk == VK_ESCAPE.0 as u32 {
            if self.edit.is_some() {
                self.edit = None;
            } else if self.input_focus {
                self.input_focus = false;
            } else if self.expanded {
                self.collapse();
            }
            self.kick();
            return true;
        }
        false
    }

    pub fn on_char(&mut self, ch: u32) {
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
                        if c.is_ascii_digit() || ":.,hml ".contains(c) {
                            if e.fresh {
                                e.buf.clear();
                                e.fresh = false;
                            }
                            if e.buf.len() < 10 {
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

    pub fn tray_cmd(&mut self, cmd: u32) {
        if cmd == tray::CMD_AUTOSTART {
            self.autostart = !self.autostart;
            tray::set_autostart(self.autostart);
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
        let want = self.user_hidden || system::fullscreen_busy();
        if want != self.hidden {
            self.hidden = want;
            unsafe {
                let _ = ShowWindow(self.hwnd, if want { SW_HIDE } else { SW_SHOWNOACTIVATE });
            }
            if !want {
                self.pop_in();
            }
        }
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

fn tab_color(t: Tab) -> C {
    match t {
        Tab::Media => WHITE,
        Tab::Focus => FOCUS_C,
        Tab::Water => WATER_C,
        Tab::Shelf => SHELF_C,
        Tab::Tasks => TASK_C,
    }
}

fn draw_tab_icon(g: &Gfx, t: Tab, r: R, col: C) {
    match t {
        Tab::Media => g.icon(ic::MUSIC, r, 14.0, col),
        Tab::Focus => g.icon(ic::TIMER, r, 14.0, col),
        Tab::Water => {
            if let Some(geo) = g.drop_geo(R::new(r.cx() - 5.5, r.cy() - 7.5, 11.0, 15.0)) {
                g.fill_geo(&geo, col);
            }
        }
        Tab::Shelf => g.icon(ic::FOLDER, r, 14.0, col),
        Tab::Tasks => g.icon(ic::TASKS, r, 14.0, col),
    }
}

fn draw_battery(g: &Gfx, x: f32, cy: f32, w: f32, h: f32, f: f32, charging: bool) {
    let body = R::new(x, cy - h / 2.0, w, h);
    g.rrect_stroke(body, 3.0, WHITE.a(0.55), 1.2, false);
    g.rrect(R::new(body.right() + 1.0, cy - h * 0.2, 2.0, h * 0.4), 1.0, WHITE.a(0.55));
    let col = if charging {
        GREEN
    } else if f <= 0.2 {
        RED
    } else {
        WHITE
    };
    let inner = body.inset(2.2, 2.2);
    g.rrect(R::new(inner.x, inner.y, (inner.w * f.clamp(0.04, 1.0)).max(1.5), inner.h), 1.5, col);
}

fn icon_btn(g: &Gfx, ui: &mut Ui, r: R, ch: char, size: f32, col: C, act: Act, bg: bool) {
    let hv = ui.hit(r, act);
    if hv || bg {
        g.rrect(r, (r.h / 2.0).min(11.0), if hv { SURF_H } else { SURF });
    }
    g.icon(ch, r, size, col);
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
    g.rrect(b, 9.0, SURF);
    let mr = R::new(bx, y, 26.0, 26.0);
    let pr = R::new(bx + box_w - 26.0, y, 26.0, 26.0);
    let mh = ui.hit(mr, minus);
    let ph = ui.hit(pr, plus);
    if mh {
        g.rrect(mr, 9.0, SURF_H);
    }
    if ph {
        g.rrect(pr, 9.0, SURF_H);
    }
    g.icon(ic::MINUS, mr, 9.0, if mh { WHITE } else { T2 });
    g.icon(ic::ADD, pr, 9.0, if ph { WHITE } else { T2 });
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
                g.line(tx + tw + 1.5, r.cy() - 6.0, tx + tw + 1.5, r.cy() + 6.0, 1.3, WHITE);
            }
        }
        None => {
            if hover {
                g.rrect(r.inset(0.0, 2.0), 7.0, WHITE.a(0.08));
            }
            g.text(val, r, size, 600, WHITE, Al::C);
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
fn parse_due(s: &str, now: i64) -> Option<Option<i64>> {
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
    let day = now.div_euclid(86400) * 86400;
    let mut t = day + h * 3600 + m * 60;
    if t <= now {
        t += 86400;
    }
    Some(Some(t))
}

fn ext_color(ext: &str) -> C {
    match ext {
        "PDF" => C::hex(0xE5484D),
        "DOC" | "DOCX" | "RTF" | "TXT" | "MD" => C::hex(0x3E7BFA),
        "XLS" | "XLSX" | "CSV" => C::hex(0x30A46C),
        "PPT" | "PPTX" | "KEY" => C::hex(0xF76B15),
        "PNG" | "JPG" | "JPEG" | "GIF" | "WEBP" | "BMP" | "SVG" | "HEIC" => C::hex(0x8E4EC6),
        "ZIP" | "RAR" | "7Z" | "TAR" | "GZ" => C::hex(0xC9A227),
        "MP3" | "WAV" | "FLAC" | "M4A" | "OGG" => C::hex(0xD6409F),
        "MP4" | "MOV" | "MKV" | "AVI" | "WEBM" => C::hex(0x12A594),
        "EXE" | "MSI" => C::hex(0x6E6E73),
        _ => C::hex(0x5B5B66),
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
    use super::{parse_amount, parse_due};

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
