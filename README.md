# linux-mouse-fix

Smooth scrolling and mouse button gestures for GNOME on Wayland, inspired by
[Mac Mouse Fix](https://github.com/noah-nuebling/mac-mouse-fix). It is an
independent project: it shares no code with Mac Mouse Fix and isn't affiliated with it.

## Features

- **Smooth, accelerated scrolling.** Each wheel notch becomes a short glide of
  two-finger touchpad scrolling, which GTK, Firefox, Chrome and other apps animate
  smoothly. Spinning the wheel quickly scrolls further.
- **Button gestures.** Each button (wheel press, both side buttons) can do something
  different on click, double click, hold, hold and drag, and hold and scroll:

  | Gesture | Options |
  |---|---|
  | Click / double click / hold | middle, left or right click, back, forward, overview, app grid, show desktop, workspace left/right, any keyboard shortcut |
  | Hold and drag | **Workspaces and overview**: a three-finger swipe that follows the mouse. Left/right slides between workspaces, up opens the overview. **Scroll and navigate**: a two-finger swipe that grabs the page, with sideways swipes for back/forward |
  | Hold and scroll | zoom, switch apps, switch workspaces, app grid and desktop, precise (slow) scroll, swift (fast) scroll |

- **Settings app.** "Mouse Fix" in your app grid. Changes apply within a second,
  with no restart and no password.

### Default buttons

| | Click | Double click | Hold | Drag | Hold + scroll |
|---|---|---|---|---|---|
| Wheel press | Middle click | – | Show desktop | Workspaces and overview | Switch apps |
| Front side button | Middle click | Overview | – | Workspaces and overview | Zoom |
| Back side button | Back | – | – | Scroll and navigate | App grid and desktop |

## Requirements

- GNOME on Wayland (tested on Ubuntu 26.04, GNOME 50)
- Python 3.11 or newer
- `python3-evdev`, plus `python3-gi`, `gir1.2-gtk-4.0` and `gir1.2-adw-1` for the settings app

```sh
sudo apt install python3-evdev python3-gi gir1.2-gtk-4.0 gir1.2-adw-1
```

## Install

```sh
git clone https://github.com/Sarwan-09/linux-mouse-fix.git
cd linux-mouse-fix
./install.sh
```

Run `./uninstall.sh` to remove it. Your settings in `~/.config/linux-mouse-fix/` are kept.

## How it works

`linux-mouse-fix` runs as a root system service. It grabs the mouse exclusively and
forwards its events through a virtual mouse, changing them along the way. Scrolling
and drags are sent to two virtual touchpads, so GNOME's own touchpad gestures do the
animation. Shortcuts are sent through a virtual keyboard.

Settings are read from `~/.config/linux-mouse-fix/config.toml` (see
`config.example.toml` for every option) and re-read whenever the file changes. Set
`enabled = false` to hand the mouse back untouched.

Because GNOME sees virtual touchpads, a Touchpad section may appear in GNOME Settings.
Touchpad scrolling follows GNOME's touchpad natural-scrolling setting. If scrolling
goes the wrong way, turn on **Reverse direction** in the app.

## Tests

```sh
tests/run_all.sh
```

The tests drive the gesture logic, scrolling and settings handling with simulated
input. They need no mouse, root or GNOME session.

## License

MIT, see [LICENSE](LICENSE).
