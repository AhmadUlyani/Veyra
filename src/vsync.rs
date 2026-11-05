//! Frame pacing for animations. A tiny thread waits for the display's vertical blank and
//! posts one `WM_APP_FRAME` per refresh while an animation runs, so opening, closing and
//! sliding between features follow the panel's refresh rate (60, 90, 120, 144, 165 Hz...)
//! instead of the ~64 Hz `SetTimer` cap. When the island is still the thread is parked
//! and costs nothing.
//!
//! Waiting order: `D3DKMTWaitForVerticalBlankEvent` (gdi32, no GPU driver/device needed),
//! then `DwmFlush`, then a plain sleep.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;
use std::thread::Thread;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_FRAME: u32 = WM_APP + 5;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
static REOPEN: AtomicBool = AtomicBool::new(false);
static HWND_: AtomicIsize = AtomicIsize::new(0);
static THREAD: OnceLock<Thread> = OnceLock::new();

pub fn start(hwnd: HWND) {
    HWND_.store(hwnd.0 as isize, Ordering::Relaxed);
    if let Ok(h) = std::thread::Builder::new().name("vsync".into()).stack_size(64 * 1024).spawn(run_loop) {
        let _ = THREAD.set(h.thread().clone());
    }
}

/// Start (true) or stop (false) posting frames.
pub fn run(on: bool) {
    let was = ACTIVE.swap(on, Ordering::AcqRel);
    if on && !was {
        if let Some(t) = THREAD.get() {
            t.unpark();
        }
    }
}

/// Whether the pacing thread is up (otherwise the caller falls back to a timer).
pub fn available() -> bool {
    THREAD.get().is_some()
}

/// Called by the window when it handles a frame, allowing the next one to be posted.
pub fn frame_done() {
    PENDING.store(false, Ordering::Release);
}

/// Monitor layout changed: re-open the adapter of the primary display.
pub fn display_changed() {
    REOPEN.store(true, Ordering::Release);
}

#[repr(C)]
struct OpenFromHdc {
    hdc: HDC,
    adapter: u32,
    luid: LUID,
    source: u32,
}

#[repr(C)]
struct WaitVBlank {
    adapter: u32,
    device: u32,
    source: u32,
}

type OpenFn = unsafe extern "system" fn(*mut OpenFromHdc) -> i32;
type WaitFn = unsafe extern "system" fn(*const WaitVBlank) -> i32;
type CloseFn = unsafe extern "system" fn(*const u32) -> i32;
type FlushFn = unsafe extern "system" fn() -> HRESULT;

enum Waiter {
    Kmt { wait: WaitFn, close: CloseFn, adapter: u32, source: u32 },
    Dwm(FlushFn),
    Sleep,
}

impl Waiter {
    fn open(skip_kmt: bool) -> Waiter {
        unsafe {
            if !skip_kmt && std::env::var_os("VEYRA_NOKMT").is_none() {
                if let Some(w) = open_kmt() {
                    return w;
                }
            }
            if let Ok(m) = LoadLibraryW(w!("dwmapi.dll")) {
                if let Some(f) = GetProcAddress(m, s!("DwmFlush")) {
                    return Waiter::Dwm(std::mem::transmute::<_, FlushFn>(f));
                }
            }
        }
        Waiter::Sleep
    }

    /// Block until the next refresh. False if the wait failed.
    fn wait(&self) -> bool {
        unsafe {
            match self {
                Waiter::Kmt { wait, adapter, source, .. } => {
                    let a = WaitVBlank { adapter: *adapter, device: 0, source: *source };
                    wait(&a) >= 0
                }
                Waiter::Dwm(f) => f().is_ok(),
                Waiter::Sleep => {
                    std::thread::sleep(Duration::from_millis(8));
                    true
                }
            }
        }
    }

    fn is_kmt(&self) -> bool {
        matches!(self, Waiter::Kmt { .. })
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if let Waiter::Kmt { close, adapter, .. } = self {
            unsafe {
                let _ = close(adapter);
            }
        }
    }
}

unsafe fn open_kmt() -> Option<Waiter> {
    let gdi = LoadLibraryW(w!("gdi32.dll")).ok()?;
    let open: OpenFn = std::mem::transmute(GetProcAddress(gdi, s!("D3DKMTOpenAdapterFromHdc"))?);
    let wait: WaitFn = std::mem::transmute(GetProcAddress(gdi, s!("D3DKMTWaitForVerticalBlankEvent"))?);
    let close: CloseFn = std::mem::transmute(GetProcAddress(gdi, s!("D3DKMTCloseAdapter"))?);
    // the island lives on the primary monitor
    let hm = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(hm, &mut mi.monitorInfo).as_bool() {
        return None;
    }
    let hdc = CreateDCW(PCWSTR::null(), PCWSTR(mi.szDevice.as_ptr()), PCWSTR::null(), None);
    if hdc.is_invalid() {
        return None;
    }
    let mut o = OpenFromHdc { hdc, adapter: 0, luid: LUID::default(), source: 0 };
    let st = open(&mut o);
    let _ = DeleteDC(hdc);
    if st < 0 {
        return None;
    }
    Some(Waiter::Kmt { wait, close, adapter: o.adapter, source: o.source })
}

fn name(w: &Waiter) -> &'static str {
    match w {
        Waiter::Kmt { .. } => "vblank",
        Waiter::Dwm(_) => "dwmflush",
        Waiter::Sleep => "sleep",
    }
}

fn run_loop() {
    let mut w = Waiter::open(false);
    crate::app::dbg_log(&format!("vsync: {}", name(&w)));
    // consecutive waits that returned (almost) instantly: a broken source would spin
    let mut fast = 0u32;
    loop {
        if !ACTIVE.load(Ordering::Acquire) {
            std::thread::park();
            continue;
        }
        if REOPEN.swap(false, Ordering::AcqRel) {
            w = Waiter::open(false);
            fast = 0;
        }
        let t = Instant::now();
        let ok = w.wait();
        if !ok || t.elapsed() < Duration::from_micros(300) {
            fast += 1;
            if fast >= 4 {
                // fall back one step: KMT -> DWM -> sleep
                w = if w.is_kmt() { Waiter::open(true) } else { Waiter::Sleep };
                crate::app::dbg_log(&format!("vsync: fell back to {}", name(&w)));
                fast = 0;
            }
            if !ok {
                std::thread::sleep(Duration::from_millis(4));
            }
        } else {
            fast = 0;
        }
        if ACTIVE.load(Ordering::Acquire) && !PENDING.swap(true, Ordering::AcqRel) {
            unsafe {
                let h = HWND(HWND_.load(Ordering::Relaxed) as *mut _);
                if PostMessageW(h, WM_APP_FRAME, WPARAM(0), LPARAM(0)).is_err() {
                    PENDING.store(false, Ordering::Release);
                }
            }
        }
    }
}
