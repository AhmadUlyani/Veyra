# Veyra

A lightweight Dynamic Island for Windows, written in Rust (pure Win32 + Direct2D, no webview).
Uses ~8 MB private memory and ~0.1% CPU while idle. The single `.exe` is about 560 KB.

## Features

| Tab | What it does |
| --- | --- |
| Music | Shows what's playing in any player that reports to Windows media controls (Spotify, Apple Music, browsers/YouTube, …). Play/pause, previous/next, click the bar to seek. Scroll on the island to change volume. |
| Lyrics | The speech-bubble button splits the island: live synced lyrics from [lrclib.net](https://lrclib.net). Click a line to jump to it. |
| Focus | Pomodoro timer (default 25/5), countdown in the compact pill, soft chime when a phase ends. |
| Water | Bottle size, daily goal, reminder interval. Reminders run 07:00–23:00 until you reach the goal. |
| Shelf | Drag files onto the island to keep them, drag them back out into chats, emails or folders. Click a file to open it. |
| Tasks | Add tasks with an optional time (− / + or the mouse wheel, ±15 / ±5 min). The island pings you when a task is due. |

System notices: volume and brightness HUD, Caps Lock, keyboard language changes,
charger plugged/unplugged, low battery, Wi-Fi connected or lost, audio output/headphones changed.
The island hides itself while a full-screen app or presentation is running.

## Usage

* Click the pill to expand it. Click the top bar, press `Esc`, or click elsewhere to collapse it.
* Right-click the island or the tray icon for **Hide / Show Veyra**, **Start with Windows** and **Quit**.
  Left-click the tray icon to bring a hidden island back.
* Veyra adds itself to the Start menu on first run, so you can launch it from Windows Search.
  Launching it again while it runs (or is hidden) just opens the island.
* Data is stored in `%APPDATA%\Veyra\data.json`.

## Build

```
powershell -ExecutionPolicy Bypass -File build.ps1
```

This runs `cargo build --release` and stamps the generated logo into `target\release\veyra.exe`
(plain `cargo build --release` works too, the exe just keeps a generic icon).
`veyra.exe --write-icon out.ico` exports the logo as a multi-size .ico.

Or just double-click `rebuild.bat`: it closes the running Veyra, builds, and starts the new build.

For screenshots and testing, `VEYRA_DEMO=media|lyrics|focus|water|shelf|tasks|toast|volume|focuspill|caps|capsoff`
opens a specific view on start (demo mode never saves data). `VEYRA_DEBUG=1` writes a log to `%TEMP%\veyra.log`.
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
