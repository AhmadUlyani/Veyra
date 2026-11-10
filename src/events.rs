//! Cross-thread event queue. Background threads (media worker, COM callbacks,
//! keyboard hook) push events here and wake the UI thread with a window message.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::lyrics::Line;
use crate::media::{Art, MediaState};
use crate::thumbs::Thumb;

pub const WM_APP_EVENT: u32 = WM_APP + 1;

pub static HWND_MAIN: AtomicIsize = AtomicIsize::new(0);
static QUEUE: Mutex<Vec<Event>> = Mutex::new(Vec::new());

pub enum Event {
    Media(Option<MediaState>),
    Art(Option<Art>),
    Lyrics(u64, Option<Vec<Line>>),
    Volume(f32, bool),
    AudioDevice,
    NetChanged,
    Caps,
    /// something droppable is over the island: true for text (-> Clipboard), false for files
    DragEnter(bool),
    DragLeave,
    /// a mouse button went down somewhere on the screen (see system::watch_clicks)
    MouseDown,
    Drop(Vec<String>),
    DropText(String),
    Thumb(String, Option<Thumb>),
}

pub fn post(e: Event) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push(e);
    }
    let h = HWND_MAIN.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as *mut _), WM_APP_EVENT, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn drain() -> Vec<Event> {
    match QUEUE.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => Vec::new(),
    }
}
