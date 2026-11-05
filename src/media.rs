//! Media worker: talks to the Windows Global System Media Transport Controls,
//! which every modern player (Spotify, Apple Music, browsers, ...) reports to.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Storage::Streams::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::WinRT::*;
use windows::Win32::UI::Shell::SHCreateMemStream;

use crate::events::{post, Event};

type Session = GlobalSystemMediaTransportControlsSession;

/// Like `.get()`, but gives up after `ms`. Calls into a player that is shutting down can
/// stall for seconds; without a limit that would freeze the worker and keep a closed
/// player on the island.
fn wait<T: RuntimeType + 'static>(op: windows::Foundation::IAsyncOperation<T>, ms: u64) -> Result<T> {
    use windows::Foundation::AsyncStatus;
    let start = Instant::now();
    loop {
        if op.Status()? != AsyncStatus::Started {
            return op.GetResults();
        }
        if start.elapsed() >= Duration::from_millis(ms) {
            let _ = op.Cancel();
            return Err(Error::from(windows::Win32::Foundation::E_ABORT));
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

/// How often the current session is re-read while one is shown (events can be missed
/// when a player quits, this keeps the island from holding on to it).
const RECHECK: Duration = Duration::from_millis(1000);
/// Poll interval while playing, for noticing a closed player (see `player.rs`).
const QUICK: Duration = Duration::from_millis(250);

pub enum MediaCmd {
    Refresh,
    Resub,
    PlayPause,
    Next,
    Prev,
    Seek(f64),
}

#[derive(Clone)]
pub struct MediaState {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app: String,
    pub playing: bool,
    pub pos: f64,
    pub dur: f64,
    pub at: Instant,
    pub can_seek: bool,
    pub key: u64,
}

pub struct Art {
    pub key: u64,
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
    pub color: (f32, f32, f32),
}

pub fn spawn() -> Sender<MediaCmd> {
    let (tx, rx) = channel();
    let t = tx.clone();
    std::thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        loop {
            match run(&t, &rx) {
                Ok(()) => break,
                Err(_) => std::thread::sleep(Duration::from_secs(5)),
            }
        }
    });
    tx
}

struct Worker {
    mgr: GlobalSystemMediaTransportControlsSessionManager,
    tx: Sender<MediaCmd>,
    unsubs: Vec<Box<dyn FnOnce()>>,
    cur: Option<Session>,
    start_ticks: i64,
    last_key: u64,
    art_key: u64,
    art_since: Instant,
    art_fallback: u64,
    playing: bool,
    had: bool,
    /// fingerprint of the last state sent to the UI, so periodic re-checks stay silent
    sent: u64,
    /// notices a player that was closed but still claims to be playing
    watch: crate::player::Watch,
    /// such a player is being hidden from the island
    gone: bool,
    last_full: Instant,
}

fn run(tx: &Sender<MediaCmd>, rx: &Receiver<MediaCmd>) -> Result<()> {
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
    {
        let t = tx.clone();
        mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            let _ = t.send(MediaCmd::Resub);
            Ok(())
        }))?;
    }
    {
        let t = tx.clone();
        mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
            let _ = t.send(MediaCmd::Resub);
            Ok(())
        }))?;
    }
    let mut w = Worker {
        mgr,
        tx: tx.clone(),
        unsubs: Vec::new(),
        cur: None,
        start_ticks: 0,
        last_key: 0,
        art_key: 0,
        art_since: Instant::now(),
        art_fallback: 0,
        playing: false,
        had: false,
        sent: 0,
        watch: crate::player::Watch::new(),
        gone: false,
        last_full: Instant::now(),
    };
    w.resub();
    w.refresh();
    loop {
        let missing_art = w.had && w.art_key != w.last_key && w.art_since.elapsed() < ART_RETRY_FOR;
        // while something plays, look a few times a second whether its player was closed
        let watching = w.watch.pending() && !w.gone;
        let timeout = if w.playing || watching {
            QUICK
        } else if w.had || w.gone || missing_art {
            RECHECK
        } else {
            Duration::from_secs(4)
        };
        match rx.recv_timeout(timeout) {
            Ok(first) => {
                // small debounce: players tend to fire bursts of events
                std::thread::sleep(Duration::from_millis(40));
                let mut cmds = vec![first];
                while let Ok(c) = rx.try_recv() {
                    cmds.push(c);
                }
                let (mut refresh, mut resub) = (false, false);
                for c in cmds {
                    match c {
                        MediaCmd::Refresh => refresh = true,
                        MediaCmd::Resub => resub = true,
                        other => {
                            w.exec(other);
                            // give the player a moment to apply the command before re-reading
                            std::thread::sleep(Duration::from_millis(120));
                            refresh = true;
                        }
                    }
                }
                if resub {
                    w.resub();
                    refresh = true;
                }
                if refresh {
                    w.refresh();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let due = w.last_full.elapsed() >= RECHECK;
                let quick = (w.playing && w.watch.suspicious()) || watching;
                if ((w.had || w.gone || missing_art) && due) || quick {
                    w.refresh();
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn is_playing(s: &Session) -> bool {
    s.GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false)
}

fn filetime_now() -> i64 {
    let ft = unsafe { GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

pub fn pretty_app(id: &str) -> String {
    let l = id.to_lowercase();
    let name = if l.contains("spotify") {
        "Spotify"
    } else if l.contains("applemusic") || l.contains("apple music") {
        "Apple Music"
    } else if l.contains("chrome") {
        "Chrome"
    } else if l.contains("msedge") || l.contains("edge") {
        "Edge"
    } else if l.contains("firefox") || l == "308046b0af4a39cb" {
        "Firefox"
    } else if l.contains("opera") {
        "Opera"
    } else if l.contains("brave") {
        "Brave"
    } else if l.contains("zune") || l.contains("media player") {
        "Media Player"
    } else if l.contains("vlc") {
        "VLC"
    } else if l.contains("tidal") {
        "TIDAL"
    } else if l.contains("deezer") {
        "Deezer"
    } else if l.contains("youtube") {
        "YouTube Music"
    } else {
        let base = id.rsplit(['\\', '/', '!']).next().unwrap_or(id);
        let base = base.trim_end_matches(".exe").trim_end_matches(".EXE");
        return base.to_string();
    };
    name.to_string()
}

impl Worker {
    fn pick(&self) -> Option<Session> {
        let cur = self.mgr.GetCurrentSession().ok();
        if let Some(c) = &cur {
            if is_playing(c) {
                return cur;
            }
        }
        let mut first = None;
        if let Ok(list) = self.mgr.GetSessions() {
            let n = list.Size().unwrap_or(0);
            for i in 0..n {
                if let Ok(s) = list.GetAt(i) {
                    if is_playing(&s) {
                        return Some(s);
                    }
                    if first.is_none() {
                        first = Some(s);
                    }
                }
            }
        }
        cur.or(first)
    }

    fn resub(&mut self) {
        for u in self.unsubs.drain(..) {
            u();
        }
        let Ok(list) = self.mgr.GetSessions() else { return };
        let n = list.Size().unwrap_or(0);
        for i in 0..n {
            let Ok(s) = list.GetAt(i) else { continue };
            let t = self.tx.clone();
            if let Ok(tok) = s.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = t.send(MediaCmd::Refresh);
                Ok(())
            })) {
                let s2 = s.clone();
                self.unsubs.push(Box::new(move || {
                    let _ = s2.RemoveMediaPropertiesChanged(tok);
                }));
            }
            let t = self.tx.clone();
            if let Ok(tok) = s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                let _ = t.send(MediaCmd::Refresh);
                Ok(())
            })) {
                let s2 = s.clone();
                self.unsubs.push(Box::new(move || {
                    let _ = s2.RemovePlaybackInfoChanged(tok);
                }));
            }
            let t = self.tx.clone();
            if let Ok(tok) = s.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = t.send(MediaCmd::Refresh);
                Ok(())
            })) {
                let s2 = s.clone();
                self.unsubs.push(Box::new(move || {
                    let _ = s2.RemoveTimelinePropertiesChanged(tok);
                }));
            }
        }
    }

    fn refresh(&mut self) {
        self.last_full = Instant::now();
        let Some(s) = self.pick() else {
            self.cur = None;
            self.playing = false;
            if self.had {
                self.had = false;
                self.art_key = 0;
                self.last_key = 0;
                self.sent = 0;
                post(Event::Media(None));
            }
            return;
        };
        match self.read(&s) {
            Some(st) => {
                self.playing = st.playing;
                self.had = true;
                let mut h = DefaultHasher::new();
                (st.key, st.playing, st.can_seek, &st.app, &st.album).hash(&mut h);
                (st.dur.to_bits(), (st.pos * 2.0).round() as i64).hash(&mut h);
                let sig = h.finish();
                // while paused nothing moves, so a re-check that found no change is not sent
                if sig != self.sent || st.playing {
                    self.sent = sig;
                    post(Event::Media(Some(st)));
                }
            }
            None => {
                self.playing = false;
                if self.had {
                    self.had = false;
                    self.art_key = 0;
                    self.last_key = 0;
                    self.sent = 0;
                    post(Event::Media(None));
                }
            }
        }
        self.cur = Some(s);
    }

    fn read(&mut self, s: &Session) -> Option<MediaState> {
        let props = wait(s.TryGetMediaPropertiesAsync().ok()?, 1500).ok()?;
        let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        if title.trim().is_empty() {
            return None;
        }
        let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
        let app = pretty_app(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());

        let info = s.GetPlaybackInfo().ok();
        if std::env::var_os("VEYRA_DEBUG").is_some() {
            let st = info.as_ref().and_then(|i| i.PlaybackStatus().ok()).map(|x| x.0).unwrap_or(-1);
            crate::app::dbg_log(&format!("media: read {app} status={st} title={title:?}"));
        }
        let playing = info
            .as_ref()
            .and_then(|i| i.PlaybackStatus().ok())
            .map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);
        let aumid = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
        self.gone = playing && self.watch.closed(&aumid);
        if !playing {
            self.watch.reset();
        }
        if self.gone {
            // closed while playing; it is only still shutting down
            return None;
        }
        let can_seek = info
            .as_ref()
            .and_then(|i| i.Controls().ok())
            .and_then(|c| c.IsPlaybackPositionEnabled().ok())
            .unwrap_or(false);

        let (mut pos, mut dur) = (0.0, 0.0);
        if let Ok(tl) = s.GetTimelineProperties() {
            let start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
            let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
            let p = tl.Position().map(|t| t.Duration).unwrap_or(0);
            self.start_ticks = start;
            dur = (end - start) as f64 / 1e7;
            pos = (p - start) as f64 / 1e7;
            if playing {
                if let Ok(last) = tl.LastUpdatedTime() {
                    let d = (filetime_now() - last.UniversalTime) as f64 / 1e7;
                    if d > 0.0 && d < 6.0 * 3600.0 {
                        pos += d;
                    }
                }
            }
            if dur > 0.0 {
                pos = pos.clamp(0.0, dur);
            }
        }

        let mut h = DefaultHasher::new();
        title.hash(&mut h);
        artist.hash(&mut h);
        let key = h.finish();
        if key != self.last_key {
            self.last_key = key;
            self.art_key = 0;
            self.art_since = Instant::now();
        }
        // Players often publish the title first and the thumbnail a moment later,
        // so keep retrying for a few seconds before giving up on the artwork.
        if self.art_key != key && self.art_since.elapsed() < ART_RETRY_FOR {
            if let Some(mut art) = load_art(&props) {
                art.key = key;
                self.art_key = key;
                post(Event::Art(Some(art)));
            } else if self.art_fallback != key && self.art_since.elapsed() > Duration::from_millis(1500) {
                // The player didn't publish artwork (Spotify sometimes doesn't, browsers often
                // don't): look the cover up online once, on a separate thread.
                self.art_fallback = key;
                let (t, a) = (title.clone(), artist.clone());
                std::thread::spawn(move || {
                    unsafe {
                        let _ = RoInitialize(RO_INIT_MULTITHREADED);
                    }
                    let r = online_art(&t, &a);
                    crate::app::dbg_log(&format!("online art for '{t}': {}", if r.is_some() { "ok" } else { "not found" }));
                    if let Some(mut art) = r {
                        art.key = key;
                        post(Event::Art(Some(art)));
                    }
                });
            }
        }

        Some(MediaState {
            title,
            artist,
            album,
            app,
            playing,
            pos,
            dur: dur.max(0.0),
            at: Instant::now(),
            can_seek,
            key,
        })
    }

    fn exec(&self, c: MediaCmd) {
        let Some(s) = &self.cur else { return };
        let _ = match c {
            MediaCmd::PlayPause => s.TryTogglePlayPauseAsync().and_then(|o| o.get()),
            MediaCmd::Next => s.TrySkipNextAsync().and_then(|o| o.get()),
            MediaCmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.get()),
            MediaCmd::Seek(sec) => s
                .TryChangePlaybackPositionAsync(self.start_ticks + (sec * 1e7) as i64)
                .and_then(|o| o.get()),
            _ => Ok(false),
        };
    }
}

fn load_art(props: &GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<Art> {
    match try_load_art(props) {
        Ok(a) => Some(a),
        Err(stage) => {
            crate::app::dbg_log(&format!("  art failed at: {stage}"));
            None
        }
    }
}

fn try_load_art(props: &GlobalSystemMediaTransportControlsSessionMediaProperties) -> std::result::Result<Art, &'static str> {
    let r = props.Thumbnail().map_err(|_| "no thumbnail")?;
    let s = r.OpenReadAsync().and_then(|o| wait(o, 2000)).map_err(|_| "open stream")?;
    let size = s.Size().map_err(|_| "size")? as u32;
    if size == 0 || size > 16 * 1024 * 1024 {
        return Err("empty stream");
    }
    let input = s.GetInputStreamAt(0).map_err(|_| "input stream")?;
    let reader = DataReader::CreateDataReader(&input).map_err(|_| "reader")?;
    let got = reader
        .LoadAsync(size)
        .and_then(|o| o.cast::<windows::Foundation::IAsyncOperation<u32>>())
        .and_then(|o| wait(o, 2000))
        .map_err(|_| "load bytes")?;
    let mut buf = vec![0u8; got as usize];
    reader.ReadBytes(&mut buf).map_err(|_| "read bytes")?;
    unsafe { decode(&buf) }.ok_or("decode image")
}

const ART: u32 = 96;
const ART_RETRY_FOR: Duration = Duration::from_secs(30);

fn http_bytes(client: &windows::Web::Http::HttpClient, url: &str) -> Option<Vec<u8>> {
    let uri = windows::Foundation::Uri::CreateUri(&HSTRING::from(url)).ok()?;
    let resp = client.GetAsync(&uri).ok()?.get().ok()?;
    if resp.StatusCode().ok()? != windows::Web::Http::HttpStatusCode::Ok {
        return None;
    }
    let buf = resp.Content().ok()?.ReadAsBufferAsync().ok()?.get().ok()?;
    let len = buf.Length().ok()? as usize;
    if len == 0 || len > 16 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::FromBuffer(&buf).ok()?;
    let mut out = vec![0u8; len];
    reader.ReadBytes(&mut out).ok()?;
    Some(out)
}

/// Cover art from the iTunes Search API (free, no key) when the player provides none.
fn online_art(title: &str, artist: &str) -> Option<Art> {
    use crate::lyrics::{clean_artist, strip_brackets};
    let t = strip_brackets(title, true);
    let a = clean_artist(artist);
    let term = format!("{a} {t}");
    let esc = windows::Foundation::Uri::EscapeComponent(&HSTRING::from(term.trim())).ok()?.to_string();
    let client = windows::Web::Http::HttpClient::new().ok()?;
    let url = format!("https://itunes.apple.com/search?term={esc}&entity=song&limit=5");
    let body = http_bytes(&client, &url)?;
    let v: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let results = v.get("results")?.as_array()?;
    let al = a.to_lowercase();
    let pick = results
        .iter()
        .find(|r| {
            let ra = r.get("artistName").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
            !al.is_empty() && (ra.contains(&al) || al.contains(&ra))
        })
        .or_else(|| results.first())?;
    let art_url = pick.get("artworkUrl100")?.as_str()?.replace("100x100bb", "300x300bb");
    let img = http_bytes(&client, &art_url)?;
    unsafe { decode(&img) }
}

unsafe fn decode(bytes: &[u8]) -> Option<Art> {
    let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let stream = SHCreateMemStream(Some(bytes))?;
    let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = dec.GetFrame(0).ok()?;
    let (mut w, mut h) = (0u32, 0u32);
    frame.GetSize(&mut w, &mut h).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let side = w.min(h);
    let clip = f.CreateBitmapClipper().ok()?;
    let rc = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
    clip.Initialize(&frame, &rc).ok()?;
    let sc = f.CreateBitmapScaler().ok()?;
    sc.Initialize(&clip, ART, ART, WICBitmapInterpolationModeFant).ok()?;
    let conv = f.CreateFormatConverter().ok()?;
    conv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
        .ok()?;
    let mut px = vec![0u8; (ART * ART * 4) as usize];
    conv.CopyPixels(std::ptr::null(), ART * 4, &mut px).ok()?;

    // average colour -> a vivid accent that reads well on black
    let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
    let mut n = 0f32;
    for p in px.chunks_exact(4) {
        let (pb, pg, pr) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let mx = pr.max(pg).max(pb);
        let mn = pr.min(pg).min(pb);
        let wgt = 0.15 + (mx - mn) / 255.0; // favour saturated pixels
        r += pr * wgt;
        g += pg * wgt;
        b += pb * wgt;
        n += wgt;
    }
    let (mut r, mut g, mut b) = (r / n / 255.0, g / n / 255.0, b / n / 255.0);
    let mx = r.max(g).max(b).max(0.01);
    let k = 0.92 / mx;
    r = (r * k).min(1.0);
    g = (g * k).min(1.0);
    b = (b * k).min(1.0);
    let mix = |v: f32| v + (1.0 - v) * 0.22;
    Some(Art { key: 0, w: ART, h: ART, px, color: (mix(r), mix(g), mix(b)) })
}
