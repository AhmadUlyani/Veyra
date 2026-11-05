//! Is a desktop player still really there? Some players (Spotify) keep reporting "playing"
//! to Windows for a few seconds after their window was closed, while they shut down. Their
//! window and their audio stream go away right at the start, so those tell us much sooner.

use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Executable behind a media session's app id, for desktop apps we can look at.
pub fn exe_of(aumid: &str) -> Option<String> {
    let l = aumid.to_lowercase();
    if l.ends_with(".exe") {
        return l.rsplit(['\\', '/']).next().map(str::to_string);
    }
    // the Microsoft Store build of Spotify still runs Spotify.exe
    if l.contains("spotify") {
        return Some("spotify.exe".into());
    }
    None
}

pub struct Watch {
    exe: Option<String>,
    pids: Vec<u32>,
    pids_at: Option<Instant>,
    /// since when the player has looked closed (no window, no sound) while saying "playing"
    gone_since: Option<Instant>,
}

/// The player must look closed for this long before we believe it.
const GONE_AFTER: Duration = Duration::from_millis(300);

impl Watch {
    pub fn new() -> Watch {
        Watch { exe: None, pids: Vec::new(), pids_at: None, gone_since: None }
    }

    fn set_exe(&mut self, exe: Option<String>) {
        if exe != self.exe {
            self.exe = exe;
            self.pids_at = None;
            self.gone_since = None;
        }
    }

    fn refresh_pids(&mut self, max_age: Duration) {
        let Some(exe) = &self.exe else { return };
        if self.pids_at.map(|t| t.elapsed() < max_age).unwrap_or(false) {
            return;
        }
        self.pids = pids_of(exe);
        self.pids_at = Some(Instant::now());
    }

    /// Cheap check for the polling loop: true when the player has no window left and no
    /// sound, i.e. a full re-read is worth doing now.
    pub fn suspicious(&mut self) -> bool {
        if self.exe.is_none() {
            return false;
        }
        self.refresh_pids(Duration::from_secs(3));
        !has_window(&self.pids) && !audio_active(&self.pids)
    }

    /// Called for a session that reports "playing": has its player actually been closed?
    pub fn closed(&mut self, aumid: &str) -> bool {
        self.set_exe(exe_of(aumid));
        if self.exe.is_none() {
            return false;
        }
        self.refresh_pids(Duration::from_millis(900));
        let window = has_window(&self.pids);
        let sound = !window && audio_active(&self.pids);
        let looks_gone = self.pids.is_empty() || (!window && !sound);
        if !looks_gone {
            self.gone_since = None;
            return false;
        }
        let since = *self.gone_since.get_or_insert_with(Instant::now);
        crate::app::dbg_log(&format!(
            "player: {} looks closed for {} ms (procs={} window={window} sound={sound})",
            self.exe.as_deref().unwrap_or("?"),
            since.elapsed().as_millis(),
            self.pids.len()
        ));
        since.elapsed() >= GONE_AFTER
    }

    /// The session isn't "playing" (or is gone): forget any half-finished suspicion.
    pub fn reset(&mut self) {
        self.gone_since = None;
    }

    pub fn pending(&self) -> bool {
        self.gone_since.is_some()
    }
}

fn pids_of(exe: &str) -> Vec<u32> {
    let mut v = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return v };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                    v.push(e.th32ProcessID);
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    v
}

struct Find<'a> {
    pids: &'a [u32],
    found: bool,
}

unsafe extern "system" fn enum_cb(h: HWND, lp: LPARAM) -> BOOL {
    let f = &mut *(lp.0 as *mut Find);
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    // a visible, titled top-level window (minimized windows count as visible)
    if f.pids.contains(&pid) && IsWindowVisible(h).as_bool() && GetWindowTextLengthW(h) > 0 {
        f.found = true;
        return FALSE;
    }
    TRUE
}

fn has_window(pids: &[u32]) -> bool {
    if pids.is_empty() {
        return false;
    }
    let mut f = Find { pids, found: false };
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut f as *mut Find as isize));
    }
    f.found
}

/// Does any of these processes have an active audio stream on any output device?
/// (When this can't be checked, assume it does, so nothing is hidden by mistake.)
fn audio_active(pids: &[u32]) -> bool {
    if pids.is_empty() {
        return false;
    }
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return true };
        let Ok(devs) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return true };
        for d in 0..devs.GetCount().unwrap_or(0) {
            let Ok(dev) = devs.Item(d) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(list) = mgr.GetSessionEnumerator() else { continue };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(c) = list.GetSession(i) else { continue };
                let Ok(c2) = c.cast::<IAudioSessionControl2>() else { continue };
                if pids.contains(&c2.GetProcessId().unwrap_or(0))
                    && c.GetState().map(|s| s == AudioSessionStateActive).unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }
    false
}
