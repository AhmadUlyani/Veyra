# Veyra

A lightweight Dynamic Island for Windows, written in Rust (pure Win32 + Direct2D, no webview).
Uses a few MB of memory and ~0.1% CPU while idle (buffers and idle pages are handed back to
Windows a few seconds after the island returns to its small shape). Animations are paced to the
display's refresh rate (60/120/144/165 Hz…) by waiting for vertical blank. The single `.exe` is about 1.1 MB.

## Features

| Tab | What it does |
| --- | --- |
| Music | Shows what's playing in any player that reports to Windows media controls (Spotify, Apple Music, browsers/YouTube, …). Play/pause, previous/next, click the bar to seek. Scroll on the island to change volume. |
| Lyrics | The speech-bubble button splits the island: live synced lyrics from [lrclib.net](https://lrclib.net). Click a line to jump to it. |
| Focus | Pomodoro timer (default 25/5), countdown in the compact pill, soft chime when a phase ends. |
| Water | Bottle size, daily goal, reminder interval. Reminders run 07:00–23:00 until you reach the goal; switch them off with the Reminders toggle. |
| Shelf | Drag files onto the island to keep them, drag them back out into chats, emails or folders. Click a file to open it. With many files, scroll to slide through them. New screenshots (saved in Pictures\Screenshots by Win+PrtScn or the Snipping Tool) are added automatically; switch that off in the tray menu. |
| Tasks | Add tasks with an optional time (− / + or the mouse wheel, ±15 / ±5 min). The island pings you when a task is due. |
| Clipboard | The last 30 pieces of text you copied, newest first. Click one to copy it again (it keeps its place), or remove it with ✕. Kept in memory only, so the list is gone when Veyra closes, and copies that apps mark as private (password managers) are skipped. Text only, no images. |

System notices: volume and brightness HUD, Caps Lock, keyboard language changes,
charger plugged/unplugged, low battery, Wi-Fi connected or lost, audio output/headphones changed, text copied.
The island hides itself while a full-screen app or presentation is running.

On the first run after installing, **Start with Windows** and **Add screenshots to Shelf** are on.
Both can be switched off in the menu, and stay off after that.

## Usage

* Click the pill to expand it (buttons react when the mouse button is released, so holding it down does nothing).
  It collapses by itself shortly after the mouse leaves it, or with `Esc` or a click outside; clicks inside
  the island never close it. It stays open while you type in a field or hold the mouse button.
* Right-click the island or the tray icon for **Hide / Show Veyra**, **Shrink to a line until hovered**,
  **Add screenshots to Shelf**, **Start with Windows** and **Quit**.
  Left-click the tray icon to bring a hidden island back.
* **Shrink to a line until hovered** (off by default): the closed island becomes a thin line at the top of the
  screen. Move the mouse onto the line, or within about 1 cm of it, and the normal pill comes back.
  Notices still show as usual.
* Veyra adds itself to the Start menu on first run, so you can launch it from Windows Search.
  Launching it again while it runs (or is hidden) just opens the island.
* Data is stored in `%APPDATA%\Veyra\data.json`.

## UI colors

Right-click the island or the tray icon → **UI colors (theme.conf)...** opens `%APPDATA%\Veyra\theme.conf`
in Notepad. Save it and Veyra applies the change within a second. Lines starting with `#` are comments.

```ini
# solid or gradient
mode = gradient
solid = #000000
# from the BOTTOM of the island (0) to the TOP (1): color position, ...
gradient = #F07A8A 0, #F2B5BC 0.45, #F2DCE0 0.75
# auto = white on dark backgrounds, dark on light ones
text = auto
# faded text (hints, labels, inactive icons): auto, normal, high, max, or 0-100
text_contrast = auto

# while the island is open: same, solid or gradient (it fades between the two)
expanded_mode = same
expanded_solid = #000000
expanded_gradient = #0A1A4A 0, #1E4FD8 0.6, #4A8BFF 1
# same, auto (picked for the open background) or a color; contrast takes text_contrast's values or same
expanded_text = auto
expanded_text_contrast = auto

# regular buttons (auto = translucent, follows the text color)
button = auto
button_hover = auto
button_text = auto
# the selected button (the open tab); not affected by button_hover
button_active = auto
# main colored buttons (Start focus, + Bottle, add task, Deselect); auto = each feature's color
button_accent = auto
# text and icons on them; auto = black or white, whichever reads better
button_accent_text = auto

# regular icons (auto = the text color); the open tab and switched-on toggles (auto = feature color)
icon = auto
icon_active = auto

# auto = the color taken from the album art
# song progress and volume bars, the moving bars in the closed island, a lyric line under the mouse
audio_bar = auto
spectrum = auto
lyrics_hover = auto

# focus timer: while focusing and during a break (auto = orange and green)
timer = auto
timer_break = auto
```

* Colors are `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the last two digits set transparency, e.g. `#FFFFFF66`).
* Gradient positions are optional (`#F07A8A, #F2DCE0` spreads the colors evenly); `45%` works too.
* Invalid values fall back to the default. An older `theme.json` is converted on first start
  (and kept as `theme.json.old`).
* New options are added to an existing `theme.conf` automatically; values you already set are left alone.
* On light backgrounds, built-in feature colors used for text and icons are darkened so they stay readable.

## Build

```
powershell -ExecutionPolicy Bypass -File build.ps1
```

This runs `cargo build --release` and stamps the generated logo into `target\release\veyra.exe`
(plain `cargo build --release` works too, the exe just keeps a generic icon).
`veyra.exe --write-icon out.ico` exports the logo as a multi-size .ico.

Or just double-click `rebuild.bat`: it closes the running Veyra, builds, and starts the new build.

For screenshots and testing, `VEYRA_DEMO=media|lyrics|focus|water|shelf|tasks|clipboard|clipboardempty|tasksempty|toast|volume|focuspill|caps|capsoff|tws|charging|copied|idle|line`
opens a specific view on start (demo mode never saves data). `VEYRA_DEMO=stress` keeps opening and closing the island
to check for leaks (`VEYRA_STRESS=fetch|toggle|water`, `VEYRA_STRESS_MS`, `VEYRA_STRESS_N`). `VEYRA_MOUSE=x,y` pretends
the cursor rests there, to see hovers. `VEYRA_DEBUG=1` writes a log to `%TEMP%\veyra.log`.
`veyra.exe --quit` asks a running Veyra to close (with its closing animation, saving data first).

## Tests

* `cargo test --release`: unit tests.
* `powershell -ExecutionPolicy Bypass -File tools\test_ui.ps1`: closes and reopens Veyra while measuring
  the notch on screen (closing/opening animation), and renders the Caps Lock pill to `target\test\*.png`.
* `tools\notch_watch.ps1 [seconds]`: prints the notch size on screen whenever it changes.
* `cargo run --example media_probe -- <seconds> [play]`: logs Windows media-session events with timestamps
  (with `play`: mutes the output, starts playback, and pauses + restores the mute at the end).
* `cargo run --example close_probe -- <play_s> <watch_s>`: plays muted, then logs every 50 ms the session
  status, the player's windows, processes and audio, while you close the player.
* `tools\debug_run.bat`: restarts Veyra with logging to `%TEMP%\veyra.log`.
