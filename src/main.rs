//! Mac Mouse Fix-style mouse features for GNOME on Wayland.
//!
//! Runs as root: grabs the mouse exclusively and forwards its events through a
//! virtual mouse, adding on the way:
//!
//!   * smooth, accelerated scrolling: each wheel notch becomes a short glide of
//!     two-finger scrolling on a virtual touchpad, which every app renders smoothly
//!     (Mac Mouse Fix calls this trackpad simulation);
//!   * per-button click, double click, hold, click-and-drag and click-and-scroll
//!     actions;
//!   * drags that drive a virtual touchpad, so GNOME's workspace/overview swipe
//!     (three fingers) and scroll/back-forward swipe (two fingers) follow the mouse.
//!
//! Settings live in a TOML file (--config) that is re-read whenever it changes, so the
//! settings app can edit it without a restart. enabled = false releases the mouse.

mod buttons;
mod config;
mod io;
mod scroll;
mod touchpad;

use std::cell::RefCell;
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::SystemTime;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, BusType, InputId, KeyCode, PropType,
            RelativeAxisCode as Rel, UinputAbsSetup};

use buttons::{Buttons, Outputs};
use config::Config;
use io::{EV_KEY, EV_REL, EV_SYN, SharedClock, SharedSink, SystemClock, UinputSink};
use scroll::{SmoothScroller, hires_emitter};
use touchpad::{SharedTouchpad, TouchpadScroll, VirtualTouchpad};

const NAME_PREFIX: &str = "linux-mouse-fix";
const DEFAULT_CONFIG: &str = "/etc/linux-mouse-fix.toml";
const USAGE: &str = "\
usage: linux-mouse-fix [--config FILE] [--device NAME] [--check] [--defaults] [--keys]

  --config FILE   settings file (default /etc/linux-mouse-fix.toml)
  --device NAME   only use a mouse whose name contains NAME
  --check         validate the settings file and exit
  --defaults      print the default settings as JSON and exit
  --keys          print the key names usable in shortcuts as JSON and exit";

macro_rules! info { ($($t:tt)*) => { eprintln!("INFO {}", format!($($t)*)) } }
macro_rules! warn { ($($t:tt)*) => { eprintln!("WARNING {}", format!($($t)*)) } }

/// The settings file, re-read whenever its modification time changes.
struct Settings {
    path: String,
    stamp: Option<SystemTime>,
    config: Config,
}

impl Settings {
    fn new(path: String) -> Self {
        Settings { path, stamp: None, config: config::defaults() }
    }

    /// Returns true when different, valid settings were loaded.
    fn reload(&mut self) -> bool {
        let stamp = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        if stamp.is_none() {
            info!("No {}, using defaults", self.path);
            self.config = config::defaults();
            return true;
        }
        match config::load(&self.path) {
            Ok(config) => {
                self.config = config;
                true
            }
            Err(err) => {
                eprintln!("ERROR Ignoring invalid {}: {err}", self.path);
                false
            }
        }
    }
}

/// The scroll and button handling built from one version of the settings.
struct Session {
    buttons: Buttons,
    scroller: Rc<RefCell<SmoothScroller>>,
    pad_scroll: Option<Rc<RefCell<TouchpadScroll>>>,
}

impl Session {
    fn new(config: &Config, out: &Outputs, scroll_pad: &SharedTouchpad) -> Self {
        let s = &config.scroll;
        let (emit, on_stop, pad_scroll): (Box<dyn FnMut(i32)>, Option<Box<dyn FnMut()>>, _) =
            if s.smooth == "off" {
                let mut wheel = hires_emitter(out.mouse.clone());
                let sign = if s.reverse { -1 } else { 1 };
                (Box::new(move |v| wheel(v * sign)), None, None)
            } else {
                let pad = Rc::new(RefCell::new(
                    TouchpadScroll::new(scroll_pad.clone(), out.clock.clone(), s.reverse)));
                let (p1, p2) = (pad.clone(), pad.clone());
                (Box::new(move |v| p1.borrow_mut().emit(v)),
                 Some(Box::new(move || p2.borrow_mut().stop())), Some(pad))
            };
        let scroller = Rc::new(RefCell::new(SmoothScroller::new(
            emit, on_stop, out.clock.clone(), &s.smooth, &s.speed, s.acceleration)));
        let buttons = Buttons::new(buttons::build_bindings(config, out, &scroller), out.clock.clone());
        Session { buttons, scroller, pad_scroll }
    }

    fn timeout(&self) -> Option<f64> {
        [self.buttons.timeout(), self.scroller.borrow().timeout(),
         self.pad_scroll.as_ref().and_then(|p| p.borrow().timeout())]
            .into_iter().flatten().reduce(f64::min)
    }

    fn tick(&mut self) {
        self.buttons.tick();
        self.scroller.borrow_mut().tick();
        if let Some(p) = &self.pad_scroll {
            p.borrow_mut().tick();
        }
    }

    fn close(&mut self) {
        if let Some(p) = &self.pad_scroll {
            p.borrow_mut().stop();
        }
    }
}

/// Routes one event from the real mouse.
fn handle(type_: u16, code: u16, value: i32, session: &mut Session, mouse: &SharedSink, hires: bool) {
    if type_ == EV_KEY && session.buttons.button(code, value) {
        return;
    }
    if type_ == EV_REL {
        if code == Rel::REL_X.0 || code == Rel::REL_Y.0 {
            let (dx, dy) = if code == Rel::REL_X.0 { (value, 0) } else { (0, value) };
            if session.buttons.motion(dx, dy) {
                return;
            }
        } else if code == Rel::REL_WHEEL.0 || code == Rel::REL_WHEEL_HI_RES.0 {
            if (code == Rel::REL_WHEEL.0) == hires {
                return; // use one of the pair; the scroller re-creates the other
            }
            let v120 = if hires { value } else { value * 120 };
            if !session.buttons.wheel(v120) {
                session.scroller.borrow_mut().wheel(v120, None);
            }
            return;
        }
    }
    if type_ == EV_SYN {
        session.buttons.frame();
        mouse.borrow_mut().syn();
    } else {
        mouse.borrow_mut().write(type_, code, value);
    }
}

fn find_mouse(name_filter: Option<&str>) -> Option<RawDevice> {
    evdev::raw_stream::enumerate().map(|(_, dev)| dev).find(|dev| {
        let name = dev.name().unwrap_or("");
        !name.starts_with(NAME_PREFIX)
            && !name.starts_with("wheel-drag-workspaces")
            && dev.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_MIDDLE))
            && dev.supported_relative_axes().is_some_and(|r| r.contains(Rel::REL_X))
            && name_filter.is_none_or(|f| name.to_lowercase().contains(&f.to_lowercase()))
    })
}

fn virtual_id() -> InputId {
    InputId::new(BusType::BUS_USB, 1, 1, 1)
}

fn make_keyboard() -> std::io::Result<VirtualDevice> {
    let keys: AttributeSet<KeyCode> =
        (KeyCode::KEY_ESC.0..=KeyCode::KEY_MICMUTE.0).map(KeyCode).collect();
    VirtualDevice::builder()?
        .name(&format!("{NAME_PREFIX} keyboard"))
        .input_id(virtual_id())
        .with_keys(&keys)?
        .build()
}

fn make_touchpad(name: &str) -> std::io::Result<VirtualDevice> {
    let keys: AttributeSet<KeyCode> = [KeyCode::BTN_LEFT, KeyCode::BTN_TOUCH, KeyCode::BTN_TOOL_FINGER,
                                       KeyCode::BTN_TOOL_DOUBLETAP, KeyCode::BTN_TOOL_TRIPLETAP]
        .into_iter().collect();
    let axis = |code, max, res| UinputAbsSetup::new(code, AbsInfo::new(0, 0, max, 0, 0, res));
    let props: AttributeSet<PropType> = [PropType::POINTER].into_iter().collect();
    let full_name = format!("{NAME_PREFIX} {name}");
    let mut builder = VirtualDevice::builder()?
        .name(&full_name)
        .input_id(virtual_id())
        .with_keys(&keys)?
        .with_properties(&props)?;
    for setup in [
        axis(Abs::ABS_X, touchpad::WIDTH, touchpad::RES),
        axis(Abs::ABS_Y, touchpad::HEIGHT, touchpad::RES),
        axis(Abs::ABS_MT_SLOT, 2, 0),
        axis(Abs::ABS_MT_TRACKING_ID, 65535, 0),
        axis(Abs::ABS_MT_POSITION_X, touchpad::WIDTH, touchpad::RES),
        axis(Abs::ABS_MT_POSITION_Y, touchpad::HEIGHT, touchpad::RES),
    ] {
        builder = builder.with_absolute_axis(&setup)?;
    }
    builder.build()
}

/// A virtual copy of the real mouse that its events are forwarded through.
fn make_mouse(dev: &RawDevice) -> std::io::Result<VirtualDevice> {
    let name = format!("{NAME_PREFIX} mouse");
    let mut builder = VirtualDevice::builder()?
        .name(&name)
        .input_id(dev.input_id());
    if let Some(keys) = dev.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(axes) = dev.supported_relative_axes() {
        builder = builder.with_relative_axes(axes)?;
    }
    if let Some(msc) = dev.misc_properties() {
        builder = builder.with_msc(msc)?;
    }
    builder.build()
}

/// Waits up to `timeout` seconds (None = forever) for the device to have events.
fn wait_readable(dev: &RawDevice, timeout: Option<f64>) -> std::io::Result<bool> {
    let mut fds = libc::pollfd { fd: dev.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = timeout.map_or(-1, |t| (t * 1000.0).ceil().min(i32::MAX as f64) as i32);
    // SAFETY: one valid pollfd, and its count.
    let n = unsafe { libc::poll(&mut fds, 1, ms) };
    if n < 0 {
        let err = std::io::Error::last_os_error();
        return if err.kind() == ErrorKind::Interrupted { Ok(false) } else { Err(err) };
    }
    if fds.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        return Err(std::io::Error::new(ErrorKind::NotConnected, "device went away"));
    }
    Ok(n > 0)
}

/// Grabs one mouse and handles it until it goes away or the service is disabled.
fn serve(mut dev: RawDevice, settings: &mut Settings, kbd: &SharedSink, touchpad: &SharedTouchpad,
         scroll_pad: &SharedTouchpad, clock: &SharedClock) -> std::io::Result<()> {
    let hires = dev.supported_relative_axes().is_some_and(|r| r.contains(Rel::REL_WHEEL_HI_RES));
    let mouse: SharedSink = Rc::new(RefCell::new(UinputSink::new(make_mouse(&dev)?)));
    let out = Outputs { kbd: kbd.clone(), mouse: mouse.clone(), touchpad: touchpad.clone(),
                        clock: clock.clone() };
    let mut session = Session::new(&settings.config, &out, scroll_pad);
    dev.grab()?;
    let mut next_check = clock.now() + 1.0;
    loop {
        let timeout = [session.timeout(), Some(next_check - clock.now())]
            .into_iter().flatten().reduce(f64::min).map(|t| t.max(0.0));
        if wait_readable(&dev, timeout)? {
            for ev in dev.fetch_events()? {
                handle(ev.event_type().0, ev.code(), ev.value(), &mut session, &mouse, hires);
            }
        }
        session.tick();
        if clock.now() < next_check {
            continue;
        }
        next_check = clock.now() + 1.0;
        // Swap settings only between presses, never in the middle of one.
        if session.buttons.is_idle() && settings.reload() {
            session.close();
            if !settings.config.enabled {
                info!("Disabled, releasing the mouse");
                return Ok(()); // dropping dev releases the grab
            }
            session = Session::new(&settings.config, &out, scroll_pad);
            info!("Settings reloaded");
        }
    }
}

fn run(mut settings: Settings, device_filter: Option<String>) -> std::io::Result<()> {
    let clock: SharedClock = Rc::new(SystemClock::new());
    let kbd: SharedSink = Rc::new(RefCell::new(UinputSink::new(make_keyboard()?)));
    let touchpad = VirtualTouchpad::new(Box::new(UinputSink::new(make_touchpad("touchpad")?)));
    let scroll_pad =
        VirtualTouchpad::new(Box::new(UinputSink::new(make_touchpad("scroll touchpad")?)));
    let mut waiting_logged = false;
    loop {
        settings.reload();
        if !settings.config.enabled {
            clock.sleep(1.0);
            continue;
        }
        let Some(dev) = find_mouse(device_filter.as_deref()) else {
            if !waiting_logged {
                info!("No mouse found, waiting for one to be plugged in");
                waiting_logged = true;
            }
            clock.sleep(2.0);
            continue;
        };
        waiting_logged = false;
        let name = dev.name().unwrap_or("mouse").to_string();
        info!("Using {name}");
        if let Err(err) = serve(dev, &mut settings, &kbd, &touchpad, &scroll_pad, &clock) {
            warn!("Lost {name}: {err}");
        }
        clock.sleep(1.0);
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut config_path, mut device, mut check) = (DEFAULT_CONFIG.to_string(), None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next().unwrap_or_default(),
            "--device" => device = args.next(),
            "--check" => check = true,
            "--defaults" => {
                println!("{}", serde_json::to_string(&config::defaults()).unwrap());
                return ExitCode::SUCCESS;
            }
            "--keys" => {
                println!("{}", serde_json::to_string(&config::key_names()).unwrap());
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if check {
        return match config::load(&config_path) {
            Ok(_) => {
                println!("settings OK");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        };
    }
    match run(Settings::new(config_path), device) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ERROR {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Sink;
    use crate::io::testing::{FakeClock, Recorder};

    struct Null;
    impl Sink for Null {
        fn write(&mut self, _: u16, _: u16, _: i32) {}
        fn syn(&mut self) {}
    }

    #[test]
    fn wheel_goes_to_scroller_and_legacy_wheel_is_dropped() {
        let clock = FakeClock::new(0.0);
        let mouse = Recorder::shared();
        let out = Outputs { kbd: Recorder::shared(), mouse: mouse.clone(),
                            touchpad: VirtualTouchpad::new(Box::new(Null)), clock: clock.clone() };
        let mut config = config::defaults();
        config.scroll.smooth = "off".into();
        config.scroll.acceleration = false;
        let mouse_sink: SharedSink = mouse.clone();
        let mut session = Session::new(&config, &out, &VirtualTouchpad::new(Box::new(Null)));
        handle(EV_REL, Rel::REL_WHEEL.0, 1, &mut session, &mouse_sink, true);
        handle(EV_REL, Rel::REL_WHEEL_HI_RES.0, 120, &mut session, &mouse_sink, true);
        handle(EV_REL, Rel::REL_X.0, 5, &mut session, &mouse_sink, true);
        handle(EV_SYN, 0, 0, &mut session, &mouse_sink, true);
        let frames = &mouse.borrow().frames;
        assert_eq!(frames[0], vec![(EV_REL, Rel::REL_WHEEL_HI_RES.0, 120), (EV_REL, Rel::REL_WHEEL.0, 1)]);
        assert_eq!(frames[1], vec![(EV_REL, Rel::REL_X.0, 5)]);
    }

    #[test]
    fn settings_reload_only_on_change_and_skip_bad_files() {
        let dir = std::env::temp_dir().join(format!("lmf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cfg.toml");
        let _ = std::fs::remove_file(&path);
        let mut s = Settings::new(path.to_string_lossy().into());
        assert!(!s.reload());
        assert!(s.config.enabled);
        std::fs::write(&path, "enabled = false\n").unwrap();
        assert!(s.reload());
        assert!(!s.config.enabled);
        assert!(!s.reload(), "unchanged");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "[scroll]\nspeed = \"warp\"\n").unwrap();
        assert!(!s.reload(), "invalid file ignored");
        assert!(!s.config.enabled, "kept the previous settings");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
