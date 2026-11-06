//! Clipboard history: text the user copies, newest first.
//!
//! Kept in memory only, like Windows' own clipboard history: it is gone after a restart and
//! copied passwords never reach the disk. Copies that password managers and other apps mark
//! as private (the standard "exclude from history" clipboard formats) are skipped.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Most entries kept; older ones fall off the end.
pub const MAX_ITEMS: usize = 30;
/// Longest text kept per entry (characters); longer copies are cut.
const MAX_CHARS: usize = 20_000;

pub struct Item {
    pub text: String,
    /// when it was copied (seconds, same clock as `util::now()`)
    pub at: i64,
}

/// Asks Windows to send WM_CLIPBOARDUPDATE to `hwnd` whenever the clipboard changes.
pub fn listen(hwnd: HWND) {
    unsafe {
        let _ = AddClipboardFormatListener(hwnd);
    }
}

pub fn stop(hwnd: HWND) {
    unsafe {
        let _ = RemoveClipboardFormatListener(hwnd);
    }
}

fn format(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

/// The current copy was marked as not for history (password managers do this).
fn private() -> bool {
    unsafe {
        for name in [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")] {
            let f = format(name);
            if f != 0 && IsClipboardFormatAvailable(f).is_ok() {
                return true;
            }
        }
        // "CanIncludeInClipboardHistory" = 0 means: keep it out of history
        let f = format(w!("CanIncludeInClipboardHistory"));
        if f != 0 && IsClipboardFormatAvailable(f).is_ok() {
            if let Ok(h) = GetClipboardData(f) {
                let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
                if !p.is_null() {
                    let v = *p;
                    let _ = GlobalUnlock(HGLOBAL(h.0));
                    return v == 0;
                }
            }
        }
        false
    }
}

/// The clipboard's text, unless there is none or it is marked private.
pub fn read(hwnd: HWND) -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() {
            return None;
        }
        // another app may hold the clipboard for a moment right after copying
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(hwnd).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            return None;
        }
        let r = (|| {
            if private() {
                return None;
            }
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let max = GlobalSize(hg) / 2;
            let mut n = 0usize;
            while n < max && *p.add(n) != 0 {
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

/// Puts `text` on the clipboard.
pub fn write(hwnd: HWND, text: &str) -> bool {
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let hg = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2).ok()?;
            let p = GlobalLock(hg) as *mut u16;
            if p.is_null() {
                let _ = GlobalFree(hg);
                return None;
            }
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            let _ = GlobalUnlock(hg);
            // on success the clipboard owns the memory
            if SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(hg.0)).is_err() {
                let _ = GlobalFree(hg);
                return None;
            }
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

/// Adds a copy to the top of `list` (a repeat moves the old entry up instead of doubling it).
/// Returns false when there was nothing worth keeping.
pub fn push(list: &mut Vec<Item>, text: String, at: i64) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let text: String = if text.chars().count() > MAX_CHARS { text.chars().take(MAX_CHARS).collect() } else { text };
    // the same copy again (apps often announce one copy twice): nothing changes
    if list.first().map(|i| i.text == text).unwrap_or(false) {
        return false;
    }
    list.retain(|i| i.text != text);
    list.insert(0, Item { text, at });
    list.truncate(MAX_ITEMS);
    true
}

/// One line for the list: whitespace runs (line breaks too) become single spaces.
pub fn preview(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(200));
    for w in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
        if out.len() > 200 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history() {
        let mut v = Vec::new();
        assert!(!push(&mut v, "  \n ".into(), 1));
        assert!(push(&mut v, "a".into(), 1));
        assert!(push(&mut v, "b".into(), 2));
        assert!(push(&mut v, "a".into(), 3));
        assert_eq!(v.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(v[0].at, 3);
        assert!(!push(&mut v, "a".into(), 4), "repeating the newest entry changes nothing");
        for i in 0..100 {
            push(&mut v, i.to_string(), 4);
        }
        assert_eq!(v.len(), MAX_ITEMS);
        assert_eq!(v[0].text, "99");
    }

    #[test]
    fn previews() {
        assert_eq!(preview("  hello\r\n\tworld  "), "hello world");
        assert!(preview(&"word ".repeat(500)).len() <= 210);
    }
}
