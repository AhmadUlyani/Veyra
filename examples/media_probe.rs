//! Test tool: logs Windows media-session events with timestamps.
//! Usage: cargo run --example media_probe -- <seconds> [play]
//! `play` mutes the system output, starts playback in the current session, and restores
//! the mute state when the probe ends (used to test closing a player while it plays).
//! Prints every SessionsChanged / CurrentSessionChanged / PlaybackInfoChanged event,
//! the sessions Windows reports at that moment, and when a Spotify.exe process appears/disappears.
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

fn endpoint() -> Option<IAudioEndpointVolume> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
    }
}

fn spotify_alive() -> bool {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]);
                if n.eq_ignore_ascii_case("spotify.exe") {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn dump(mgr: &GlobalSystemMediaTransportControlsSessionManager) -> String {
    let mut out = Vec::new();
    if let Ok(list) = mgr.GetSessions() {
        for i in 0..list.Size().unwrap_or(0) {
            let Ok(s) = list.GetAt(i) else { continue };
            let id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
            let st = s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map(|x| x.0).unwrap_or(-1);
            let title = s
                .TryGetMediaPropertiesAsync()
                .and_then(|o| o.get())
                .and_then(|p| p.Title())
                .map(|h| h.to_string())
                .unwrap_or_default();
            out.push(format!("[{id} status={st} title={title:?}]"));
        }
    }
    if out.is_empty() { "(no sessions)".into() } else { out.join(" ") }
}

fn main() -> windows::core::Result<()> {
    let secs: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20);
    let t0 = Instant::now();
    let ms = move || t0.elapsed().as_millis();
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
    let (tx, rx) = channel::<&'static str>();
    let t = tx.clone();
    mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| { let _ = t.send("SessionsChanged"); Ok(()) }))?;
    let t = tx.clone();
    mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| { let _ = t.send("CurrentSessionChanged"); Ok(()) }))?;
    if let Ok(list) = mgr.GetSessions() {
        for i in 0..list.Size().unwrap_or(0) {
            if let Ok(s) = list.GetAt(i) {
                let t = tx.clone();
                let _ = s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| { let _ = t.send("PlaybackInfoChanged"); Ok(()) }));
            }
        }
    }
    let play = std::env::args().nth(2).as_deref() == Some("play");
    let vol = if play { endpoint() } else { None };
    let was_muted = vol.as_ref().map(|v| unsafe { v.GetMute().map(|b| b.as_bool()).unwrap_or(false) });
    if play {
        if let Some(v) = &vol {
            unsafe { let _ = v.SetMute(true, std::ptr::null()); }
        }
        if let Ok(s) = mgr.GetCurrentSession() {
            let _ = s.TryPlayAsync().and_then(|o| o.get());
        }
    }
    println!("{:>6} ms  start: {}  spotify.exe={}", ms(), dump(&mgr), spotify_alive());
    let mut alive = spotify_alive();
    while t0.elapsed() < Duration::from_secs(secs) {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            println!("{:>6} ms  {ev}: {}", ms(), dump(&mgr));
        }
        let a = spotify_alive();
        if a != alive {
            alive = a;
            println!("{:>6} ms  spotify.exe running = {a}   sessions: {}", ms(), dump(&mgr));
        }
    }
    println!("{:>6} ms  end: {}", ms(), dump(&mgr));
    if play {
        // never leave music playing out loud once the mute is lifted
        if let Ok(list) = mgr.GetSessions() {
            for i in 0..list.Size().unwrap_or(0) {
                if let Ok(s) = list.GetAt(i) {
                    let _ = s.TryPauseAsync().and_then(|o| o.get());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(600));
    }
    if let (Some(v), Some(m)) = (&vol, was_muted) {
        unsafe { let _ = v.SetMute(m, std::ptr::null()); }
    }
    Ok(())
}
