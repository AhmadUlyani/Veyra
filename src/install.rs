//! Makes Veyra findable in Windows Search: writes the logo as an .ico and keeps a
//! Start-menu shortcut pointing at the current executable.

use std::path::PathBuf;

use windows::core::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

use crate::util::wide;

fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Veyra"))
}

pub fn icon_path() -> Option<PathBuf> {
    Some(data_dir()?.join("veyra.ico"))
}

fn shortcut_path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("APPDATA")?)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Veyra.lnk"),
    )
}

/// Write the icon file (when missing or outdated) and (re)create the Start-menu shortcut.
pub fn ensure() {
    let Some(ico) = icon_path() else { return };
    let bytes = crate::logo::ico_bytes();
    let stale = std::fs::read(&ico).map(|old| old != bytes).unwrap_or(true);
    if stale {
        if let Some(dir) = data_dir() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&ico, &bytes);
    }
    let (Ok(exe), Some(lnk)) = (std::env::current_exe(), shortcut_path()) else { return };
    unsafe {
        let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { return };
        let exe_w = wide(&exe.to_string_lossy());
        let dir_w = wide(&exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
        let ico_w = wide(&ico.to_string_lossy());
        let _ = link.SetPath(PCWSTR(exe_w.as_ptr()));
        let _ = link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()));
        let _ = link.SetDescription(w!("Veyra - a Dynamic Island for Windows"));
        let _ = link.SetIconLocation(PCWSTR(ico_w.as_ptr()), 0);
        if let Ok(pf) = link.cast::<IPersistFile>() {
            let lnk_w = wide(&lnk.to_string_lossy());
            let _ = pf.Save(PCWSTR(lnk_w.as_ptr()), true);
        }
    }
}

/// `veyra.exe --write-icon <path>`: used by build.ps1 to stamp the icon into the exe.
pub fn write_icon(path: &str) {
    let _ = std::fs::write(path, crate::logo::ico_bytes());
}
