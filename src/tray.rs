use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util::wide;

pub const WM_APP_TRAY: u32 = WM_APP + 2;
pub const CMD_AUTOSTART: u32 = 10;
pub const CMD_QUIT: u32 = 11;
pub const CMD_HIDE: u32 = 12;
pub const CMD_THEME: u32 = 13;
pub const CMD_LINE: u32 = 14;
pub const CMD_SHOTS: u32 = 15;

/// Check marks shown in the menu.
pub struct MenuState {
    pub autostart: bool,
    pub hidden: bool,
    pub line_mode: bool,
    pub screenshots: bool,
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

pub fn add(hwnd: HWND, icon: HICON) {
    let mut n = nid(hwnd);
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    n.uCallbackMessage = WM_APP_TRAY;
    n.hIcon = icon;
    let tip: Vec<u16> = "Veyra".encode_utf16().collect();
    n.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &n);
    }
}

pub fn remove(hwnd: HWND) {
    let n = nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &n);
    }
}

pub fn menu(hwnd: HWND, st: &MenuState) -> u32 {
    let check = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        let _ = AppendMenuW(m, MF_STRING | MF_GRAYED, 0, w!("Veyra"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let hide = if st.hidden { w!("Show Veyra") } else { w!("Hide Veyra") };
        let _ = AppendMenuW(m, MF_STRING, CMD_HIDE as usize, hide);
        let _ = AppendMenuW(m, check(st.line_mode), CMD_LINE as usize, w!("Shrink to a line until hovered"));
        let _ = AppendMenuW(m, check(st.screenshots), CMD_SHOTS as usize, w!("Add screenshots to Shelf"));
        let _ = AppendMenuW(m, check(st.autostart), CMD_AUTOSTART as usize, w!("Start with Windows"));
        let _ = AppendMenuW(m, MF_STRING, CMD_THEME as usize, w!("UI colors (theme.conf)..."));
        let _ = AppendMenuW(m, MF_STRING, CMD_QUIT as usize, w!("Quit Veyra"));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, p.x, p.y, 0, hwnd, None);
        let _ = DestroyMenu(m);
        cmd.0 as u32
    }
}

/// Opens a config file in Notepad (always present, and .json may have no default app).
pub fn edit_file(path: &std::path::Path) {
    let args = wide(&format!("\"{}\"", path.display()));
    unsafe {
        ShellExecuteW(HWND::default(), w!("open"), w!("notepad.exe"), PCWSTR(args.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, w!("Veyra"), RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let v = wide(&format!("\"{}\"", exe.display()));
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN,
                w!("Veyra"),
                REG_SZ.0,
                Some(v.as_ptr() as *const _),
                (v.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, w!("Veyra"));
        }
    }
}
