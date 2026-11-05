//! Test tool: what happens, moment by moment, when a player is closed.
//! Usage: cargo run --example close_probe -- <play_seconds> <watch_seconds>
//! Mutes the output, starts playback, plays for <play_seconds>, then prints a line every time
//! one of these changes (sampled every 50 ms) until <watch_seconds> after that:
//!   session status / title, number of visible Spotify windows, Spotify process count,
//!   whether Spotify's audio session is active.
//! Close Spotify (e.g. with its X button) during the watch period. At the end it pauses
//! anything still playing and restores the mute state.
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Media::Control::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn spotify_pids() -> Vec<u32> {
    let mut v = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return v };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]);
                if n.eq_ignore_ascii_case("spotify.exe") {
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

static mut PIDS: Vec<u32> = Vec::new();
static mut VIS: u32 = 0;
unsafe extern "system" fn enum_cb(h: HWND, _: LPARAM) -> BOOL {
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    #[allow(static_mut_refs)]
    if PIDS.contains(&pid) && IsWindowVisible(h).as_bool() && GetWindowTextLengthW(h) > 0 {
        VIS += 1;
    }
    TRUE
}
fn visible_windows(pids: &[u32]) -> u32 {
    unsafe {
        PIDS = pids.to_vec();
        VIS = 0;
        let _ = EnumWindows(Some(enum_cb), LPARAM(0));
        VIS
    }
}

fn audio_active(mgr: &IAudioSessionManager2, pids: &[u32]) -> bool {
    unsafe {
        let Ok(en) = mgr.GetSessionEnumerator() else { return false };
        for i in 0..en.GetCount().unwrap_or(0) {
            let Ok(c) = en.GetSession(i) else { continue };
            let Ok(c2) = c.cast::<IAudioSessionControl2>() else { continue };
            let pid = c2.GetProcessId().unwrap_or(0);
            if pids.contains(&pid) && c.GetState().map(|s| s == AudioSessionStateActive).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn main() -> windows::core::Result<()> {
    let play_s: f64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(5.0);
    let watch_s: f64 = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(15.0);
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let en: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let dev = unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole)? };
    let vol: IAudioEndpointVolume = unsafe { dev.Activate(CLSCTX_ALL, None)? };
    let asm: IAudioSessionManager2 = unsafe { dev.Activate(CLSCTX_ALL, None)? };
    let was_muted = unsafe { vol.GetMute()?.as_bool() };
    unsafe { vol.SetMute(true, std::ptr::null())? };

    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
    if let Ok(s) = mgr.GetCurrentSession() {
        let _ = s.TryPlayAsync().and_then(|o| o.get());
    }
    std::thread::sleep(Duration::from_secs_f64(play_s));
    let t0 = Instant::now();
    let wall = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 100_000_000;
    println!("{} watching -- close Spotify now", wall());
    let mut last = String::new();
    while t0.elapsed().as_secs_f64() < watch_s {
        let (mut st, mut title) = (-1, String::from("-"));
        if let Ok(list) = mgr.GetSessions() {
            if let Ok(s) = list.GetAt(0) {
                st = s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map(|x| x.0).unwrap_or(-1);
                title = s.TryGetMediaPropertiesAsync().and_then(|o| o.get()).and_then(|p| p.Title()).map(|h| h.to_string()).unwrap_or_default();
            }
        }
        let pids = spotify_pids();
        let line = format!(
            "status={st} title={title:?} windows={} procs={} audio_active={}",
            visible_windows(&pids),
            pids.len(),
            audio_active(&asm, &pids)
        );
        if line != last {
            println!("{} {:>6} ms  {line}", wall(), t0.elapsed().as_millis());
            last = line;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Ok(list) = mgr.GetSessions() {
        for i in 0..list.Size().unwrap_or(0) {
            if let Ok(s) = list.GetAt(i) {
                let _ = s.TryPauseAsync().and_then(|o| o.get());
            }
        }
    }
    std::thread::sleep(Duration::from_millis(600));
    unsafe { vol.SetMute(was_muted, std::ptr::null())? };
    Ok(())
}
