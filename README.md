# Deskflow Companion

**Your monitor follows your mouse.** When [Deskflow](https://github.com/deskflow/deskflow) moves your keyboard
and mouse to another computer, Deskflow Companion (`dfc`) switches the shared monitor's input to that computer,
over DDC/CI, the same way the monitor's own menu button would.

```
   Linux PC  ───── DisplayPort ─────┐
      │                             ▼
      │ Deskflow (keyboard+mouse)  ┌─────────────┐
      │                            │   shared    │  input 0x0f = PC
      ▼                            │   monitor   │  input 0x11 = laptop
   Windows laptop ──── HDMI ──────▶└─────────────┘
```

Move the cursor off the PC's screen onto the laptop and the shared monitor shows the laptop. Move it back and
the monitor shows the PC again. No buttons, no hotkeys, no reaching for the monitor menu.

- **Linux and Windows**, one small program (`dfc`), no runtime to install.
- **Works with Deskflow as it is**: it only uses Deskflow's own `enterCommand` / `exitCommand` hooks.
- **Knows your laptop's own touchpad from Deskflow**: on Windows, moving onto the shared monitor with the
  laptop's own mouse or touchpad doesn't switch it. Only Deskflow does.
- **Tray icon** on both systems: switch by hand, pause automatic switching, settings.
- **Several shared monitors**, each with its own input codes, each switched on its own.
- **Safe to try**: `install --dry-run` shows every change first. Updates keep the previous version for a
  one-command rollback, and `uninstall` restores Deskflow's settings exactly.

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [First-time setup](#first-time-setup)
- [Everyday use](#everyday-use)
- [Configuration](#configuration)
- [Commands](#commands)
- [How it works](#how-it-works)
- [Update, roll back, uninstall](#update-roll-back-uninstall)
- [Troubleshooting](#troubleshooting)
- [Building from source](#building-from-source)
- [License](#license)

## Requirements

- A monitor connected to two (or more) computers, with **DDC/CI turned on** in its on-screen menu (most
  monitors have it, often on by default).
- [Deskflow](https://github.com/deskflow/deskflow) set up between the computers and started at least once on
  each, so its `Deskflow.conf` exists.
- **Linux**: [`ddcutil`](https://www.ddcutil.com/), and access to `/dev/i2c-*` for your user (usually the `i2c`
  group plus the `i2c-dev` kernel module; see [ddcutil's guide](https://www.ddcutil.com/i2c_permissions/)).
  For the tray icon: a desktop with StatusNotifierItem support (KDE Plasma does; GNOME needs the
  AppIndicator extension).
- **Windows**: Windows 10 or 11, 64-bit.

Check that your monitor answers before going further: on Linux `ddcutil detect` should list it.

## Install

Download the archive for each computer from the
[latest release](https://github.com/Hameed-Alahmadi/deskflow-companion/releases/latest), unpack it anywhere
and run, in a terminal in that folder:

```bash
./dfc install --dry-run   # Windows: dfc.exe install --dry-run
./dfc install             # Windows: dfc.exe install
```

`install` does the following and prints each step:

1. Copies itself to a fixed place: `~/.local/bin/dfc`, or `%LOCALAPPDATA%\DeskflowCompanion\dfc.exe`.
2. Writes a default config file if there is none.
3. Stops Deskflow, sets its `enterCommand` / `exitCommand` to `dfc enter` / `dfc leave`, and starts it again.
   Before editing, it saves a timestamped backup of `Deskflow.conf` and remembers the original lines for
   `uninstall`. Pass `--no-restart` to leave Deskflow running and restart it yourself.
4. Starts the background part at every login:
   - **Linux**: the tray (`dfc tray`), via `~/.config/autostart/deskflow-companion.desktop`.
   - **Windows**: the daemon (`dfc daemon`, with the tray icon), via the scheduled task `DeskflowCompanion`.

The archive can be deleted afterwards.

## First-time setup

dfc needs to know two things: **which monitor is shared**, and **which input code belongs to which computer**.

**1. Find the shared monitor.** Run `dfc detect` on each computer:

```
$ dfc detect
* /dev/i2c-4 (card1-DP-2) [Monitor 1]
    BNQ BenQ EX2710S, serial AB000000001
    input: 0x0f (pc)

* = shared monitor per config: /home/you/.config/deskflow-companion/config.json
```

The line with `*` is the monitor dfc will switch. The default config takes the only DDC/CI monitor (Linux) or
the only non-primary display (Windows). If that's the wrong one, or you have several, choose it:

- **Windows**: tray icon → **Settings…** → *Display*.
- **Linux**: tray icon → **Shared monitors** → tick it.

Either way its serial number and model are saved, so it's recognised no matter which port or position it is on.

**2. Learn the input codes.** Each computer is plugged into a different input of the monitor, and each input
has a number (a VCP 0x60 code). With the monitor showing the PC, `dfc detect` (or *Its input now* in the
Windows settings) shows that input's code. Switch the monitor to the laptop with its own buttons and look
again. Common values:

| Input | Typical code |
|---|---|
| DisplayPort 1 / 2 | `0x0f` / `0x10` |
| HDMI 1 / 2 | `0x11` / `0x12` |
| USB-C | `0x1b` (varies) |

Enter them in the settings (Windows) or the config file (Linux: tray → **Edit config file…**), for example
`"inputs": { "pc": "0x0f", "laptop": "0x11" }`, and set **This machine** to the right name on each computer.

**3. Try it.** `dfc switch laptop` and `dfc switch pc` should flip the monitor. Then move the mouse between
the computers with Deskflow.

**Optional: a hotkey.** The Windows daemon registers a hotkey (default `Ctrl+Alt+F13`) that moves the cursor
onto the shared monitor and switches it. With a Deskflow server on Linux you can send it from a keyboard
shortcut in Deskflow's layout file, for example:

```
section: options
	keystroke(Control+Alt+Left) = switchToScreen(laptop), keystroke(Control+Alt+F13,laptop)
end
```

## Everyday use

There's nothing to do: the monitor follows Deskflow. The tray icon shows the state: **blue** means it
switches automatically, **grey** means it's paused.

| Tray menu | |
|---|---|
| **Switch to** *machine* | Switches every shared monitor that has an input for that machine, right now, even while paused. With several monitors there's a submenu per monitor too. |
| **Switch automatically** | Untick to pause: Deskflow (and the Windows hotkey) stop switching this computer. Each computer has its own setting. |
| **Settings…** (Windows) | Every option in one window. A left click on the icon opens it too. |
| **Shared monitors**, **This machine** (Linux) | Pick the shared displays and this computer's name. |
| **Edit config file…** (Linux) | Opens the config file, for the input codes and everything else. |
| **Open log** | Every switch, and why. |
| **Exit / Quit** | Stops dfc until the next login. |

Changes take effect within a couple of seconds, whether they come from the settings, the tray or a hand edit
of the config file. If the file is broken, dfc keeps using the last good version and says so in the tooltip and
the log.

## Configuration

One JSON file per computer:

- Linux: `~/.config/deskflow-companion/config.json`
- Windows: `%APPDATA%\DeskflowCompanion\config.json`

Every field is optional; missing ones take the defaults.

```json
{
  "this_machine": "pc",
  "auto_switch": true,
  "monitors": [
    {
      "name": "Desk monitor",
      "serial": "AB000000001",
      "model": "EX2710S",
      "inputs": { "pc": "0x0f", "laptop": "0x11" }
    }
  ],
  "hotkey": "Ctrl+Alt+F13",
  "physical_input_window_ms": 300
}
```

| Field | Meaning |
|---|---|
| `this_machine` | Which entry in `inputs` is this computer. `dfc enter` switches to it. |
| `auto_switch` | `false` pauses switching by Deskflow and the hotkey. |
| `monitors` | The shared monitors. |
| `monitors[].name` | Label in menus and the log (default: the model). |
| `monitors[].serial`, `.model` | How the monitor is recognised, from its EDID: serial first, then model (a partial, case-insensitive match, so a replacement unit of the same model still matches). |
| `monitors[].position` | Windows only, for displays without EDID: `left` / `right` of the primary display, or `primary`. |
| `monitors[].inputs` | Input code per computer name. A monitor without an entry for this computer is left alone on it. |
| `hotkey` | Windows: modifiers (`Ctrl`, `Alt`, `Shift`, `Win`) plus one key (`F1`–`F24`, `A`–`Z`, `0`–`9`). |
| `physical_input_window_ms` | Windows: how recent a real mouse/touchpad movement must be to count as "the local mouse moved it". |

**Several monitors.** Each shared monitor switches on its own. On Windows, only the monitor the cursor enters
switches, and the hotkey takes the first one. On Linux, Deskflow only says "the cursor is back", not where, so
every shared monitor switches back.

## Commands

```
dfc detect                      list monitors, their current input, which ones are shared
dfc switch <machine> [monitor]  switch the shared monitors that have that machine (or just one)
dfc switch 0xNN [monitor]       switch to a raw input code
dfc enter / dfc leave           Deskflow hooks (set by install)
dfc daemon                      Windows: the background part with the tray icon
dfc tray                        Linux: the tray icon
dfc settings                    Windows: the settings window
dfc install   [--dry-run] [--no-restart]
dfc uninstall [--dry-run] [--no-restart]
dfc --version
```

Switching to the input a monitor is already on does nothing (re-setting it would blank the screen for a
moment).

## How it works

**Linux.** Deskflow runs `dfc enter` when the cursor arrives on this computer. dfc asks `ddcutil` to set the
monitor's input (VCP code 0x60) to this computer's code. The monitor's I²C bus is cached, so a switch takes a
fraction of a second.

**Windows.** The daemon watches the cursor. When it lands on a shared monitor, dfc switches the monitor only if
the cursor arrived through Deskflow. That's the hard part: Windows reports the laptop's own touchpad as
"injected" input, just like Deskflow's, so the input type alone can't tell them apart. Instead `dfc enter` /
`dfc leave` (Deskflow's hooks) raise and lower a flag while Deskflow holds the cursor, and any real mouse
movement in the last few hundred milliseconds vetoes the switch. Switching uses the Windows monitor
configuration API (dxva2). Monitors are recognised by the EDID stored in the registry.

**Logs.** `~/.local/state/deskflow-companion.log` or `%LOCALAPPDATA%\DeskflowCompanion\dfc.log`. Each line
says what happened and why, for example:

```
entered from Deskflow (deskflow active, physical 4693 ms ago, injected 0 ms ago): 0x0f -> 0x11
entered with the local mouse, no switch (deskflow inactive, physical 0 ms ago, injected 1921 ms ago)
```

## Update, roll back, uninstall

- **Update**: run `install` from the newer download. The installed copy is replaced, and the old one is kept
  next to it as `dfc.prev` (`dfc.prev.exe`).
- **Roll back**: `dfc.prev install`.
- **Uninstall**: `dfc uninstall`. It restores Deskflow's previous `enterCommand` / `exitCommand` and removes the
  login entry (Linux: autostart file and tray; Windows: scheduled task). The program, config and log stay in
  place: delete them by hand if you like.

## Troubleshooting

| Symptom | What to check |
|---|---|
| `detect` says *unreadable* | DDC/CI is off in the monitor menu, or (Linux) no access to `/dev/i2c-*`. Some monitors don't answer for about a second after an input change. |
| `detect` finds no monitors (Linux) | Run `ddcutil detect`; fix that first (permissions, `i2c-dev` module). |
| The wrong monitor switches | Choose the shared monitor in the settings/tray. After re-cabling on Linux, delete `~/.cache/deskflow-companion/bus-*`. |
| Nothing happens on Windows | Is the task running? `schtasks /query /tn DeskflowCompanion`. Then check the log. |
| Tray icon is grey, nothing switches | Automatic switching is paused on that computer: tick **Switch automatically**. |
| No tray icon on Linux | Run `dfc tray` in a terminal to see the error. GNOME needs the AppIndicator extension. |
| Switches when using the laptop's own touchpad | Raise `physical_input_window_ms`, and send the log lines around it in an issue. |
| `install` says Deskflow runs outside its autostart unit (Linux) | Close Deskflow and run `install` again. |

## Building from source

Needs a recent Rust toolchain (edition 2024). The Windows build is cross-compiled from Linux with MinGW-w64.

```bash
cargo build --release                                  # Linux: target/release/dfc
rustup target add x86_64-pc-windows-gnu                # once
cargo build --release --target x86_64-pc-windows-gnu   # Windows: needs mingw-w64-gcc and windres
cargo test
scripts/package.sh                                     # tests, both builds, release archives in dist/
scripts/icons.sh /tmp/icons assets                     # redraw the icons (ImageMagick)
```

| Path | |
|---|---|
| `src/main.rs` | Command line |
| `src/config.rs` | Config file, reading older formats |
| `src/ddc/` | Finding monitors and switching inputs: `linux.rs` (ddcutil), `windows.rs` (dxva2 + EDID) |
| `src/winwatch.rs`, `wintray.rs`, `winsettings.rs` | Windows daemon, tray icon, settings window |
| `src/linuxtray.rs` | Linux tray (StatusNotifierItem) |
| `src/install.rs`, `src/ini.rs` | install/uninstall, `Deskflow.conf` editing |
| `assets/`, `build.rs` | Icons and the Windows resource/manifest |

## License

[MIT](LICENSE). Deskflow is a separate project; this companion is not affiliated with it.
