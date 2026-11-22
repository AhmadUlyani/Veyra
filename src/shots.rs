//! New screenshots → Shelf. Watches the Screenshots folder (Pictures\Screenshots, where both
//! Win+PrtScn and the Snipping Tool / Win+Shift+S save by default) and reports image files
//! that appear in it after Veyra started.

use std::path::PathBuf;
use std::time::SystemTime;

use windows::core::HSTRING;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};

use crate::events::{post, Event};
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

pub struct Watcher {
    dir: Option<PathBuf>,
    /// files modified at or before this are not new
    since: SystemTime,
    /// the folder's own modification time at the last scan (it changes when a file is added)
    dir_mtime: Option<SystemTime>,
}

fn folder() -> Option<PathBuf> {
    // testing aid: watch another folder instead (with VEYRA_DEMO, so nothing is saved)
    if let Some(d) = std::env::var_os("VEYRA_SHOTS_DIR") {
        return Some(PathBuf::from(d));
    }
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            if let Some(s) = s {
                return Some(PathBuf::from(s));
            }
        }
    }
    Some(PathBuf::from(std::env::var_os("USERPROFILE")?).join("Pictures").join("Screenshots"))
}

fn is_image(p: &std::path::Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
    )
}

/// Wakes the UI (Event::ShotsChanged) as soon as something changes in the Screenshots folder,
/// so a new screenshot is picked up at once instead of at the next 2-second check (which stays
/// as the fallback, e.g. when the folder does not exist yet). Waits in the kernel: no CPU while idle.
pub fn watch_changes() {
    let Some(dir) = folder() else { return };
    let _ = std::thread::Builder::new().name("shots".into()).spawn(move || unsafe {
        let w = HSTRING::from(dir.as_os_str());
        let Ok(h) = FindFirstChangeNotificationW(&w, false, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE) else {
            return;
        };
        loop {
            if WaitForSingleObject(h, INFINITE) != WAIT_OBJECT_0 {
                break;
            }
            post(Event::ShotsChanged);
            if FindNextChangeNotification(h).is_err() {
                break;
            }
        }
        let _ = FindCloseChangeNotification(h);
    });
}

impl Watcher {
    pub fn new() -> Watcher {
        Watcher::at(folder())
    }

    fn at(dir: Option<PathBuf>) -> Watcher {
        let dir_mtime = dir.as_ref().and_then(|d| std::fs::metadata(d).ok()?.modified().ok());
        Watcher { dir, since: SystemTime::now(), dir_mtime }
    }

    /// Screenshots saved since the last call (cheap when nothing changed: one metadata read).
    pub fn poll(&mut self) -> Vec<String> {
        let Some(dir) = &self.dir else { return Vec::new() };
        let m = std::fs::metadata(dir).ok().and_then(|m| m.modified().ok());
        if m.is_none() || m == self.dir_mtime {
            return Vec::new();
        }
        self.dir_mtime = m;
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut found: Vec<(SystemTime, String)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let t = e.metadata().ok()?.modified().ok()?;
                (t > self.since && is_image(&p)).then(|| (t, p.to_string_lossy().into_owned()))
            })
            .collect();
        found.sort();
        if let Some((t, _)) = found.last() {
            self.since = *t;
        }
        found.into_iter().map(|f| f.1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_new_images() {
        let dir = std::env::temp_dir().join(format!("veyra-shots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.png"), b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let mut w = Watcher::at(Some(dir.clone()));
        assert!(w.poll().is_empty(), "files from before the start are not new");
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("Screenshot 1.png"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let got = w.poll();
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("Screenshot 1.png"));
        assert!(w.poll().is_empty(), "each screenshot is reported once");
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("Screenshot 2.JPG"), b"x").unwrap();
        assert_eq!(w.poll().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
