<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="resources/logo_white.svg" />
    <source media="(prefers-color-scheme: light)" srcset="resources/logo_black.svg" />
    <img alt="StretchLock" src="resources/logo_black.svg" width="200" />
  </picture>
</p>

# StretchLock

A desktop application that forces you to take stretch breaks by displaying a fullscreen overlay on a configurable timer. Unlike conventional break reminder apps that rely on easily dismissed notifications, StretchLock puts an immovable overlay on your screen and blocks mouse input so you have no choice but to step away.

A configurable passphrase override is available for situations where you genuinely cannot step away (e.g. during a live call).

## Why StretchLock?

Most break reminder tools rely on dismissable notifications or gentle popups. When you're deep in focus, it takes a single click to ignore them -- defeating the purpose entirely.

Existing alternatives:

- **Stretchly** -- open-source, cross-platform. Shows break reminders as popups that can be skipped or postponed.
- **Big Stretch Reminder** -- Windows-only. Displays a reminder window that can be closed immediately.
- **StretchClock** -- simple tray reminder with a dismissable alert.
- **EyeLeo** -- shows animated break reminders, but they're easy to ignore.
- **Workrave** -- the most full-featured option, but its lock mode is outdated and unreliable on modern Windows.

All of these share the same fundamental flaw: they ask you to take a break. StretchLock *makes* you take a break. It covers your entire screen with a transparent overlay, blocks mouse input via a low-level system hook, and gives you no way out except waiting or typing a passphrase you deliberately chose to make inconvenient. The break screen stays transparent so you can still see your work -- nothing is hidden, you just can't touch it until the timer runs out.

## How it works

StretchLock runs silently in the background as a system tray application -- there is no main window. When you launch it, nothing visible happens on screen. The app lives in the **system tray** (the small icon area near the clock in your taskbar or panel). On Windows, you may need to click the **^** arrow in the taskbar to expand the tray and see the StretchLock icon.

From there, everything is controlled by **right-clicking the tray icon**:

- **Status line** -- shows a live countdown to the next break (e.g. "Next break in 47:32").
- **Disable / Enable** -- pause or resume the timer.
- **Settings** -- open a dialog to configure work interval, break duration, passphrase, and log level.
- **Take a break now** -- trigger an immediate break without waiting for the timer.
- **Quit** -- close the app.

When the timer runs out, StretchLock covers your entire screen with a transparent overlay. You can still see everything behind it, but your mouse is blocked -- you physically cannot click on anything. A frosted-glass card in the centre shows a countdown, a reminder to stretch, and your break stats for the day.

The only ways to end a break early are:

1. **Wait** for the countdown to finish.
2. **Type the passphrase** into the input field on the overlay and press Enter.

Once the break ends, the overlay disappears and the work timer starts again.

## Features

- Configurable work interval (1 -- 480 minutes) and break duration (1 -- 60 minutes)
- Transparent fullscreen overlay with a frosted-glass card, covering all monitors during breaks
- Mouse input blocking during breaks (keyboard remains active for passphrase entry)
- System tray menu with live countdown, enable/disable toggle, and forced break trigger
- Passphrase-based override to dismiss a break early when necessary
- Break tracking: daily count and consecutive-day streak
- Desktop notification when a break starts
- Rotating log file with configurable verbosity (DEBUG / INFO / WARNING / ERROR)
- Cross-platform: Windows 10+ and Linux (X11)

### Platform details

| | Windows | Linux (X11) |
|---|---|---|
| Mouse blocking | Low-level hook (`WH_MOUSE_LL`) | `XGrabPointer` via libX11 |
| Cursor during break | Frozen in place | Hidden and restored after break |
| Alt+Tab prevention | `BypassWindowManagerHint` | Keyboard grab + automatic focus recovery |
| Portable format | `.exe` (PyInstaller onefile) | `.AppImage` (PyInstaller + appimagetool) |

## Requirements

- **Windows**: Windows 10 or later
- **Linux**: X11-based desktop environment (most distros with GNOME on Xorg, KDE, XFCE, etc.)

No Python, no dependencies, no installation required. The portable `.exe` (Windows) or `.AppImage` (Linux) bundles everything it needs.

## Getting started

### Windows

Download `StretchLock.exe` from the [latest release](https://github.com/FilipePires98/StretchLock/releases/latest), double-click it, and find the StretchLock icon in your system tray.

### Linux

Download `StretchLock-x86_64.AppImage` from the [latest release](https://github.com/FilipePires98/StretchLock/releases/latest), make it executable, and run it:

```
chmod +x StretchLock-x86_64.AppImage
./StretchLock-x86_64.AppImage
```

A recommended permanent location is `~/Applications/`:

```
mkdir -p ~/Applications
mv StretchLock-x86_64.AppImage ~/Applications/
```

## Run on startup

### Windows

1. Place `StretchLock.exe` in a permanent location (e.g. `C:\Program Files\StretchLock\`).
2. Press `Win + R`, type `shell:startup`, and hit Enter.
3. Right-click inside the Startup folder, select **New > Shortcut**, and point it to `StretchLock.exe`.

To stop it from auto-starting, delete the shortcut from the Startup folder.

### Linux

Create an XDG autostart entry:

```
mkdir -p ~/.config/autostart

cat > ~/.config/autostart/stretchlock.desktop << 'EOF'
[Desktop Entry]
Name=StretchLock
Exec=/home/YOUR_USER/Applications/StretchLock-x86_64.AppImage
Type=Application
X-GNOME-Autostart-enabled=true
Comment=Force stretch breaks by locking your screen on a timer
EOF
```

Replace the `Exec` path with the actual location of your AppImage.

To stop it from auto-starting, delete `~/.config/autostart/stretchlock.desktop`.

## Configuration

Settings are stored in a JSON file at:

- Windows: `%APPDATA%\StretchLock\config.json`
- Linux: `~/.config/stretchlock/config.json`

Default values:

| Setting                | Default   | Range / Values                  |
|------------------------|-----------|---------------------------------|
| work_interval_minutes  | 60        | 1 -- 480                        |
| break_duration_minutes | 5         | 1 -- 60                         |
| passphrase             | unlock    | any non-empty string            |
| enabled                | true      | true / false                    |
| log_level              | INFO      | DEBUG, INFO, WARNING, ERROR     |

You can edit this file directly or use the Settings dialog from the tray menu.

## Credits

Created by Filipe Lopes Pires.
