#![windows_subsystem = "windows"]

mod app;
mod events;
mod gfx;
mod install;
mod logo;
mod lyrics;
mod media;
mod player;
mod shelf;
mod sound;
mod store;
mod system;
mod tray;
mod util;

use std::cell::RefCell;
use std::sync::atomic::Ordering;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use app::{App, Deferred};

const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static ICON: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);

        // build helper: `veyra.exe --write-icon out.ico`
        let args: Vec<String> = std::env::args().collect();
        if args.len() >= 3 && args[1] == "--write-icon" {
            install::write_icon(&args[2]);
            return;
        }

        // `veyra.exe --quit`: ask the running copy to close (used by rebuild.bat and for testing)
        if args.len() >= 2 && args[1] == "--quit" {
            if let Ok(h) = FindWindowW(w!("VeyraIsland"), PCWSTR::null()) {
                let _ = PostMessageW(h, app::WM_APP_QUIT, WPARAM(0), LPARAM(0));
            }
            return;
        }

        // Already running (e.g. launched again from Windows Search): bring it back and open it.
        let _mutex = CreateMutexW(None, BOOL::from(true), w!("Local\\Veyra.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(w!("VeyraIsland"), PCWSTR::null()) {
                // we were launched by the user, so we may hand the foreground to the running copy
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                let _ = AllowSetForegroundWindow(pid);
                let _ = PostMessageW(h, app::WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return;
        }


        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let cls = w!("VeyraIsland");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: cls,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            cls,
            w!("Veyra"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            HWND::default(),
            HMENU::default(),
            hinst,
            None,
        ) else {
            return;
        };
        events::HWND_MAIN.store(hwnd.0 as isize, Ordering::Relaxed);
        TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(w!("TaskbarCreated"))));

        let app = match App::new(hwnd) {
            Ok(a) => a,
            Err(_) => return,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));

        shelf::register(hwnd);
        install::ensure();
        let icon = logo::hicon(GetSystemMetrics(SM_CXSMICON).max(16) as u32);
        ICON.with(|c| c.set(icon.0 as isize));
        tray::add(hwnd, icon);

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                app.start();
            }
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Data was saved in WM_DESTROY. A normal exit can deadlock while COM/WinRT DLLs
        // detach with background threads still alive, leaving a windowless process that
        // blocks the next launch, so end the process directly.
        app::dbg_log("quit: loop ended");
        quit_now();
    }
}

pub fn quit_now() -> ! {
    unsafe {
        let _ = windows::Win32::System::Threading::TerminateProcess(windows::Win32::System::Threading::GetCurrentProcess(), 0);
    }
    std::process::exit(0)
}

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::DragOut(paths) => shelf::drag_out(hwnd, &paths),
        Deferred::Menu => {
            let (auto, hidden) = with_app(|a| (a.autostart, a.user_hidden)).unwrap_or((false, false));
            let cmd = tray::menu(hwnd, auto, hidden);
            if cmd == tray::CMD_QUIT {
                // plays the closing animation, then closes the window
                with_app(|a| a.begin_quit());
            } else if cmd != 0 {
                with_app(|a| a.tray_cmd(cmd));
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == TASKBAR_CREATED.with(|c| c.get()) && msg != 0 {
        tray::add(hwnd, HICON(ICON.with(|c| c.get()) as *mut _));
        return LRESULT(0);
    }
    let handled: Option<(Option<LRESULT>, Option<Deferred>)> = with_app(|app| {
        let r = match msg {
            WM_TIMER => {
                match wp.0 {
                    1 => app.frame(),
                    _ => app.tick(),
                }
                Some(LRESULT(0))
            }
            app::WM_APP_SHOW => {
                app.show_and_open();
                Some(LRESULT(0))
            }
            app::WM_APP_QUIT => {
                app.begin_quit();
                Some(LRESULT(0))
            }
            events::WM_APP_EVENT => {
                app.on_events();
                Some(LRESULT(0))
            }
            tray::WM_APP_TRAY => {
                let ev = (lp.0 & 0xFFFF) as u32;
                if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                    app.deferred = Some(Deferred::Menu);
                } else if ev == WM_LBUTTONUP {
                    app.show_and_open();
                }
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                app.on_mouse_move(lp);
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                app.on_mouse_leave();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                app.on_ldown(lp);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                app.on_lup(lp);
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                app.on_rup();
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                app.on_wheel(wp, lp);
                Some(LRESULT(0))
            }
            WM_ACTIVATE => {
                app.on_activate((wp.0 & 0xFFFF) as u32 != WA_INACTIVE);
                None
            }
            WM_KEYDOWN => {
                if app.on_key(wp.0 as u32) {
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            WM_CHAR => {
                app.on_char(wp.0 as u32);
                Some(LRESULT(0))
            }
            WM_POWERBROADCAST => {
                app.on_power(wp, lp);
                Some(LRESULT(1))
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
                app.on_display_change();
                None
            }
            WM_DESTROY => {
                app::dbg_log("quit: WM_DESTROY");
                app.save_now();
                app::dbg_log("quit: saved");
                Some(LRESULT(0))
            }
            _ => None,
        };
        (r, app.deferred.take())
    });

    if msg == WM_DESTROY {
        tray::remove(hwnd);
        PostQuitMessage(0);
        return LRESULT(0);
    }
    if let Some((r, d)) = handled {
        if let Some(d) = d {
            run_deferred(hwnd, d);
        }
        if let Some(r) = r {
            return r;
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
