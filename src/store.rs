use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Water {
    pub bottle_ml: u32,
    pub goal_ml: u32,
    pub interval_min: u32,
    /// drink reminders on/off
    pub remind: bool,
    pub date: String,
    pub log: Vec<u32>,
}

impl Default for Water {
    fn default() -> Self {
        Water { bottle_ml: 500, goal_ml: 2000, interval_min: 60, remind: true, date: String::new(), log: Vec::new() }
    }
}

impl Water {
    pub fn total(&self) -> u32 {
        self.log.iter().sum()
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Task {
    pub id: u64,
    pub text: String,
    pub due: Option<i64>,
    pub done: bool,
    pub notified: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Focus {
    pub focus_min: u32,
    pub break_min: u32,
    pub date: String,
    pub sessions: u32,
}

impl Default for Focus {
    fn default() -> Self {
        Focus { focus_min: 25, break_min: 5, date: String::new(), sessions: 0 }
    }
}

/// Switches from the tray menu.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// shrink to a thin line at the top of the screen unless the mouse is on it
    pub line_mode: bool,
    /// put new screenshots on the Shelf
    pub screenshots: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { line_mode: false, screenshots: true }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Data {
    pub water: Water,
    pub focus: Focus,
    pub tasks: Vec<Task>,
    pub shelf: Vec<String>,
    pub next_id: u64,
    pub settings: Settings,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let mut p = PathBuf::from(base);
    p.push("Veyra");
    Some(p)
}

pub fn load() -> Data {
    path()
        .and_then(|p| std::fs::read_to_string(p.join("data.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(d: &Data) {
    let Some(dir) = path() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(s) = serde_json::to_string_pretty(d) {
        let tmp = dir.join("data.json.tmp");
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("data.json"));
        }
    }
}
