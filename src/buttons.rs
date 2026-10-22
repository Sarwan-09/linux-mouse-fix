//! Per-button click / double click / hold / drag / scroll gestures.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use evdev::KeyCode;

use crate::config::{self, Config};
use crate::io::{EV_KEY, SharedClock, SharedSink};
use crate::scroll::{SmoothScroller, hires_emitter};
use crate::touchpad::{Drag, SharedTouchpad, TouchpadDrag};

pub const CLICK_SLOP: i32 = 12; // movement (mouse counts) still treated as a click
pub const DOUBLE_CLICK_WINDOW: f64 = 0.3; // s from the first release to the second press
pub const HOLD_TIME: f64 = 0.45; // s held still before the hold action fires

pub type Action = Rc<dyn Fn()>;

/// Press the keys in order, then release them in reverse.
pub fn press_keys(kbd: &SharedSink, clock: &SharedClock, keys: &[KeyCode]) {
    let presses = keys.iter().map(|k| (*k, 1)).chain(keys.iter().rev().map(|k| (*k, 0)));
    for (key, value) in presses {
        let mut k = kbd.borrow_mut();
        k.write(EV_KEY, key.0, value);
        k.syn();
        drop(k);
        clock.sleep(0.01);
    }
}

pub fn native_click(mouse: SharedSink, button: KeyCode) -> Action {
    Rc::new(move || {
        let mut m = mouse.borrow_mut();
        for value in [1, 0] {
            m.write(EV_KEY, button.0, value);
            m.syn();
        }
    })
}

fn key_action(kbd: &SharedSink, clock: &SharedClock, spec: &str) -> Action {
    let keys = config::parse_keys(spec).expect("validated");
    let (kbd, clock) = (kbd.clone(), clock.clone());
    Rc::new(move || press_keys(&kbd, &clock, &keys))
}

/// What a held button does when the wheel turns.
pub trait ScrollEffect {
    fn begin(&mut self);
    fn wheel(&mut self, v120: i32);
    fn end(&mut self);
}

/// Calls up()/down() once per wheel notch while the button is held.
struct StepScroll {
    up: Action,
    down: Action,
    on_first: Option<Action>,
    on_end: Option<Action>,
    acc: i32,
    stepped: bool,
}

impl ScrollEffect for StepScroll {
    fn begin(&mut self) {
        self.acc = 0;
        self.stepped = false;
    }

    fn wheel(&mut self, v120: i32) {
        self.acc += v120;
        while self.acc.abs() >= 120 {
            if !self.stepped {
                if let Some(f) = &self.on_first { f() }
            }
            self.stepped = true;
            if self.acc > 0 { (self.up)() } else { (self.down)() }
            self.acc -= 120 * self.acc.signum();
        }
    }

    fn end(&mut self) {
        if self.stepped {
            if let Some(f) = &self.on_end { f() }
        }
    }
}

/// Holds keys while passing the wheel through unsmoothed (ctrl -> zoom).
struct ModifierScroll {
    kbd: SharedSink,
    keys: Vec<KeyCode>,
    emit: Box<dyn FnMut(i32)>,
    scroller: Rc<RefCell<SmoothScroller>>,
}

impl ScrollEffect for ModifierScroll {
    fn begin(&mut self) {
        self.scroller.borrow_mut().stop(); // a glide still running would scroll with the keys held
        let mut k = self.kbd.borrow_mut();
        for key in &self.keys {
            k.write(EV_KEY, key.0, 1);
        }
        k.syn();
    }

    fn wheel(&mut self, v120: i32) {
        (self.emit)(v120);
    }

    fn end(&mut self) {
        let mut k = self.kbd.borrow_mut();
        for key in self.keys.iter().rev() {
            k.write(EV_KEY, key.0, 0);
        }
        k.syn();
    }
}

/// Wheel up opens the app grid, down shows the desktop; the reverse undoes it.
struct DesktopAndApps {
    grid: Action,
    desktop: Action,
    acc: i32,
    shown: Option<&'static str>,
}

impl ScrollEffect for DesktopAndApps {
    fn begin(&mut self) {
        self.acc = 0;
        self.shown = None;
    }

    fn wheel(&mut self, v120: i32) {
        self.acc += v120;
        if self.acc.abs() < 120 {
            return;
        }
        let up = self.acc > 0;
        self.acc = 0;
        match (up, self.shown) {
            (true, Some("desktop")) => { (self.desktop)(); self.shown = None }
            (true, None) => { (self.grid)(); self.shown = Some("grid") }
            (false, Some("grid")) => { (self.grid)(); self.shown = None }
            (false, None) => { (self.desktop)(); self.shown = Some("desktop") }
            _ => {}
        }
    }

    fn end(&mut self) {}
}

/// Scrolls through the smooth scroller at a fixed speed (precise / swift).
struct SpeedScroll {
    scroller: Rc<RefCell<SmoothScroller>>,
    factor: f64,
}

impl ScrollEffect for SpeedScroll {
    fn begin(&mut self) {}
    fn wheel(&mut self, v120: i32) {
        self.scroller.borrow_mut().wheel(v120, Some(self.factor));
    }
    fn end(&mut self) {}
}

/// Everything a binding's effects may need to reach.
pub struct Outputs {
    pub kbd: SharedSink,
    pub mouse: SharedSink,
    pub touchpad: SharedTouchpad,
    pub clock: SharedClock,
}

pub fn make_action(spec: Option<&String>, out: &Outputs) -> Option<Action> {
    let spec = spec?.as_str();
    if spec == "none" {
        return None;
    }
    if let Some(button) = config::lookup(config::CLICKS, spec) {
        return Some(native_click(out.mouse.clone(), button));
    }
    Some(key_action(&out.kbd, &out.clock, config::lookup(config::SHORTCUTS, spec).unwrap_or(spec)))
}

pub fn make_drag(spec: Option<&String>, out: &Outputs, scale: f64, invert: bool) -> Option<Box<dyn Drag>> {
    let new = |fingers, sx, sy| -> Box<dyn Drag> {
        Box::new(TouchpadDrag::new(out.touchpad.clone(), out.clock.clone(), fingers, sx, sy))
    };
    match spec.map(String::as_str) {
        // GNOME swipes are "natural" (content follows the fingers), so by default the
        // fingers move against the mouse: drag right -> next workspace. Up -> overview.
        Some("spaces") => Some(new(3, if invert { scale } else { -scale }, scale)),
        // Grab the page: content follows the mouse; sideways swipes go back/forward.
        Some("navigate") => Some(new(2, scale, scale)),
        _ => None,
    }
}

pub fn make_scroll(spec: Option<&String>, out: &Outputs,
                   scroller: &Rc<RefCell<SmoothScroller>>) -> Option<Box<dyn ScrollEffect>> {
    let keys = |spec: &str| key_action(&out.kbd, &out.clock, spec);
    let step = |up, down, on_first, on_end| -> Box<dyn ScrollEffect> {
        Box::new(StepScroll { up, down, on_first, on_end, acc: 0, stepped: false })
    };
    Some(match spec.map(String::as_str)? {
        "zoom" => Box::new(ModifierScroll {
            kbd: out.kbd.clone(),
            keys: vec![KeyCode::KEY_LEFTCTRL],
            emit: hires_emitter(out.mouse.clone()),
            scroller: scroller.clone(),
        }),
        "app-switcher" => {
            // Like holding Super and pressing Tab; Super goes down only on the first
            // notch, since Super pressed and released alone opens the overview.
            let hold = |value| -> Action {
                let kbd = out.kbd.clone();
                Rc::new(move || {
                    let mut k = kbd.borrow_mut();
                    k.write(EV_KEY, KeyCode::KEY_LEFTMETA.0, value);
                    k.syn();
                })
            };
            step(keys("shift+tab"), keys("tab"), Some(hold(1)), Some(hold(0)))
        }
        "spaces" => step(keys("super+pageup"), keys("super+pagedown"), None, None),
        "desktop-and-apps" => Box::new(DesktopAndApps {
            grid: keys("super+a"),
            desktop: keys("super+d"),
            acc: 0,
            shown: None,
        }),
        "precise" => Box::new(SpeedScroll { scroller: scroller.clone(), factor: 0.25 }),
        "swift" => Box::new(SpeedScroll { scroller: scroller.clone(), factor: 4.0 }),
        _ => return None,
    })
}

#[derive(Default)]
pub struct Binding {
    pub click: Option<Action>,
    pub double: Option<Action>,
    pub hold: Option<Action>,
    pub drag: Option<Box<dyn Drag>>,
    pub scroll: Option<Box<dyn ScrollEffect>>,
}

pub fn build_bindings(config: &Config, out: &Outputs,
                      scroller: &Rc<RefCell<SmoothScroller>>) -> HashMap<u16, Binding> {
    config.buttons.iter().map(|(name, spec)| {
        let button = config::lookup(config::BUTTONS, name).expect("validated");
        let click = match spec.get("click") {
            Some(_) => make_action(spec.get("click"), out),
            None => Some(native_click(out.mouse.clone(), button)),
        };
        (button.0, Binding {
            click,
            double: make_action(spec.get("double-click"), out),
            hold: make_action(spec.get("hold"), out),
            drag: make_drag(spec.get("drag"), out, config.drag.scale, config.drag.invert),
            scroll: make_scroll(spec.get("scroll"), out, scroller),
        })
    }).collect()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Undecided,
    Drag,
    Scroll,
    Hold,
}

struct Press {
    code: u16,
    start: f64,
    second: bool, // second press of a double click
    travel: i32,
    mode: Mode,
}

/// The gesture state machine. Pure logic: effects come in through the bindings.
pub struct Buttons {
    bindings: HashMap<u16, Binding>,
    clock: SharedClock,
    active: Option<Press>,
    swallowed: Vec<u16>, // buttons pressed while another one was active
    pending: Option<(u16, f64)>, // single click waiting to see if a double follows
}

impl Buttons {
    pub fn new(bindings: HashMap<u16, Binding>, clock: SharedClock) -> Self {
        Buttons { bindings, clock, active: None, swallowed: vec![], pending: None }
    }

    pub fn is_idle(&self) -> bool {
        self.active.is_none()
    }

    /// Returns true if the event was consumed.
    pub fn button(&mut self, code: u16, value: i32) -> bool {
        if !self.bindings.contains_key(&code) {
            return false;
        }
        match value {
            2 => {} // autorepeat
            1 => {
                if self.active.is_some() {
                    self.swallowed.push(code);
                    return true;
                }
                let second = matches!(self.pending, Some((c, _)) if c == code);
                if self.pending.is_some() && !second {
                    self.fire_pending();
                }
                self.pending = None;
                self.active = Some(Press { code, start: self.clock.now(), second, travel: 0,
                                           mode: Mode::Undecided });
            }
            _ => {
                if let Some(i) = self.swallowed.iter().position(|&c| c == code) {
                    self.swallowed.remove(i);
                    return true;
                }
                if self.active.as_ref().is_none_or(|p| p.code != code) {
                    return true;
                }
                let press = self.active.take().unwrap();
                let b = self.bindings.get_mut(&code).unwrap();
                match press.mode {
                    Mode::Drag => b.drag.as_mut().unwrap().end(),
                    Mode::Scroll => b.scroll.as_mut().unwrap().end(),
                    Mode::Hold => {}
                    Mode::Undecided => {
                        if press.second && b.double.is_some() {
                            (b.double.as_ref().unwrap())();
                        } else if b.double.is_some() {
                            self.pending = Some((code, self.clock.now() + DOUBLE_CLICK_WINDOW));
                        } else if let Some(click) = &b.click {
                            click();
                        }
                    }
                }
            }
        }
        true
    }

    /// Returns true if the motion was consumed (pointer frozen).
    pub fn motion(&mut self, dx: i32, dy: i32) -> bool {
        let Some(press) = &mut self.active else { return false };
        press.travel += dx.abs() + dy.abs();
        let b = self.bindings.get_mut(&press.code).unwrap();
        if press.mode == Mode::Undecided && press.travel > CLICK_SLOP {
            if let Some(drag) = &mut b.drag {
                drag.begin();
                press.mode = Mode::Drag;
            }
        }
        if press.mode == Mode::Drag {
            b.drag.as_mut().unwrap().motion(dx, dy);
            return true;
        }
        false
    }

    /// Returns true if the wheel event was consumed.
    pub fn wheel(&mut self, v120: i32) -> bool {
        let Some(press) = &mut self.active else { return false };
        let b = self.bindings.get_mut(&press.code).unwrap();
        let Some(scroll) = &mut b.scroll else { return false };
        match press.mode {
            Mode::Undecided => {
                scroll.begin();
                press.mode = Mode::Scroll;
            }
            Mode::Scroll => {}
            _ => return false,
        }
        scroll.wheel(v120);
        true
    }

    pub fn frame(&mut self) {
        if let Some(press) = &self.active {
            if press.mode == Mode::Drag {
                self.bindings.get_mut(&press.code).unwrap().drag.as_mut().unwrap().frame();
            }
        }
    }

    fn hold_due(&self) -> Option<f64> {
        let press = self.active.as_ref()?;
        let b = &self.bindings[&press.code];
        (press.mode == Mode::Undecided && b.hold.is_some() && press.travel <= CLICK_SLOP)
            .then_some(press.start + HOLD_TIME)
    }

    pub fn timeout(&self) -> Option<f64> {
        let due = [self.pending.map(|p| p.1), self.hold_due()].into_iter().flatten()
            .reduce(f64::min)?;
        Some((due - self.clock.now()).max(0.0))
    }

    pub fn tick(&mut self) {
        let now = self.clock.now();
        if matches!(self.pending, Some((_, due)) if now >= due) {
            self.fire_pending();
        }
        if self.hold_due().is_some_and(|due| now >= due) {
            let press = self.active.as_mut().unwrap();
            press.mode = Mode::Hold;
            (self.bindings[&press.code].hold.as_ref().unwrap())();
        }
    }

    fn fire_pending(&mut self) {
        if let Some((code, _)) = self.pending.take() {
            if let Some(click) = &self.bindings[&code].click {
                click();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::testing::{FakeClock, Recorder};

    const MIDDLE: u16 = KeyCode::BTN_MIDDLE.0;
    const FRONT: u16 = KeyCode::BTN_EXTRA.0;
    const BACK: u16 = KeyCode::BTN_SIDE.0;

    type Log = Rc<RefCell<Vec<String>>>;

    fn note(log: &Log, s: &str) -> Action {
        let (log, s) = (log.clone(), s.to_string());
        Rc::new(move || log.borrow_mut().push(s.clone()))
    }

    struct FakeDrag(Log);
    impl Drag for FakeDrag {
        fn begin(&mut self) { self.0.borrow_mut().push("drag-begin".into()) }
        fn motion(&mut self, _: i32, _: i32) {}
        fn frame(&mut self) {}
        fn end(&mut self) { self.0.borrow_mut().push("drag-end".into()) }
    }

    struct FakeScroll(Log);
    impl ScrollEffect for FakeScroll {
        fn begin(&mut self) { self.0.borrow_mut().push("scroll-begin".into()) }
        fn wheel(&mut self, v: i32) { self.0.borrow_mut().push(format!("wheel {v}")) }
        fn end(&mut self) { self.0.borrow_mut().push("scroll-end".into()) }
    }

    fn setup() -> (Buttons, Log, Rc<FakeClock>) {
        let log: Log = Rc::default();
        let clock = FakeClock::new(0.0);
        let mut b = HashMap::new();
        b.insert(MIDDLE, Binding {
            click: Some(note(&log, "mclick")),
            hold: Some(note(&log, "hold")),
            drag: Some(Box::new(FakeDrag(log.clone()))),
            scroll: Some(Box::new(FakeScroll(log.clone()))),
            ..Default::default()
        });
        b.insert(FRONT, Binding {
            click: Some(note(&log, "fclick")),
            double: Some(note(&log, "double")),
            drag: Some(Box::new(FakeDrag(log.clone()))),
            ..Default::default()
        });
        b.insert(BACK, Binding { click: Some(note(&log, "back")), ..Default::default() });
        (Buttons::new(b, clock.clone()), log, clock)
    }

    fn tap(b: &mut Buttons, code: u16) {
        b.button(code, 1);
        b.button(code, 0);
    }

    fn take(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    #[test]
    fn click_hold_drag_scroll() {
        let (mut b, log, clock) = setup();
        tap(&mut b, MIDDLE);
        assert_eq!(take(&log), ["mclick"]);

        clock.set(1.0);
        b.button(MIDDLE, 1);
        assert!((b.timeout().unwrap() - HOLD_TIME).abs() < 1e-9);
        clock.set(1.5);
        b.tick();
        b.button(MIDDLE, 0);
        assert_eq!(take(&log), ["hold"]);

        clock.set(2.0);
        b.button(MIDDLE, 1);
        assert!(b.motion(20, 0));
        clock.set(3.0);
        b.tick();
        b.button(MIDDLE, 0);
        assert_eq!(take(&log), ["drag-begin", "drag-end"]);

        b.button(MIDDLE, 1);
        assert!(b.wheel(120));
        assert!(b.wheel(-120));
        b.button(MIDDLE, 0);
        assert_eq!(take(&log), ["scroll-begin", "wheel 120", "wheel -120", "scroll-end"]);
    }

    #[test]
    fn unbound_parts_pass_through() {
        let (mut b, log, _) = setup();
        assert!(!b.wheel(120));
        b.button(BACK, 1);
        assert!(!b.motion(50, 0), "no drag effect: pointer moves");
        assert!(!b.wheel(120), "no scroll effect: wheel scrolls");
        b.button(BACK, 0);
        assert_eq!(take(&log), ["back"]);
        assert!(!b.button(KeyCode::BTN_LEFT.0, 1));
    }

    #[test]
    fn double_click() {
        let (mut b, log, clock) = setup();
        clock.set(10.0);
        tap(&mut b, FRONT);
        b.tick();
        assert!(take(&log).is_empty(), "single click waits");
        clock.set(10.31);
        b.tick();
        assert_eq!(take(&log), ["fclick"]);

        clock.set(11.0);
        tap(&mut b, FRONT);
        clock.set(11.1);
        tap(&mut b, FRONT);
        clock.set(12.0);
        b.tick();
        assert_eq!(take(&log), ["double"]);

        clock.set(13.0);
        tap(&mut b, FRONT);
        clock.set(13.1);
        tap(&mut b, MIDDLE);
        assert_eq!(take(&log), ["fclick", "mclick"], "another button flushes the wait");

        clock.set(14.0);
        tap(&mut b, FRONT);
        clock.set(14.1);
        b.button(FRONT, 1);
        b.motion(50, 0);
        b.button(FRONT, 0);
        clock.set(15.0);
        b.tick();
        assert_eq!(take(&log), ["drag-begin", "drag-end"]);
    }

    #[test]
    fn second_button_while_one_is_held_is_swallowed() {
        let (mut b, log, _) = setup();
        b.button(MIDDLE, 1);
        tap(&mut b, BACK);
        b.button(MIDDLE, 0);
        assert_eq!(take(&log), ["mclick"]);
    }

    fn outputs() -> (Outputs, Rc<RefCell<Recorder>>, Rc<RefCell<Recorder>>) {
        struct Null;
        impl crate::io::Sink for Null {
            fn write(&mut self, _: u16, _: u16, _: i32) {}
            fn syn(&mut self) {}
        }
        let kbd = Recorder::shared();
        let mouse = Recorder::shared();
        let out = Outputs {
            kbd: kbd.clone(),
            mouse: mouse.clone(),
            touchpad: crate::touchpad::VirtualTouchpad::new(Box::new(Null)),
            clock: FakeClock::new(0.0),
        };
        (out, kbd, mouse)
    }

    fn scroller(out: &Outputs) -> Rc<RefCell<SmoothScroller>> {
        Rc::new(RefCell::new(SmoothScroller::new(Box::new(|_| {}), None, out.clock.clone(),
                                                 "high", "medium", true)))
    }

    fn pressed_keys(rec: &Rc<RefCell<Recorder>>) -> Vec<(u16, i32)> {
        rec.borrow().events().into_iter().map(|(_, c, v)| (c, v)).collect()
    }

    #[test]
    fn app_switcher_holds_super_from_the_first_full_notch() {
        let (out, kbd, _) = outputs();
        let mut sw = make_scroll(Some(&"app-switcher".into()), &out, &scroller(&out)).unwrap();
        sw.begin();
        sw.wheel(60);
        assert!(kbd.borrow().events().is_empty(), "half a notch: Super not pressed yet");
        sw.wheel(60);
        sw.wheel(-120);
        sw.end();
        use KeyCode as K;
        assert_eq!(pressed_keys(&kbd), [
            (K::KEY_LEFTMETA.0, 1),
            (K::KEY_LEFTSHIFT.0, 1), (K::KEY_TAB.0, 1), (K::KEY_TAB.0, 0), (K::KEY_LEFTSHIFT.0, 0),
            (K::KEY_TAB.0, 1), (K::KEY_TAB.0, 0),
            (K::KEY_LEFTMETA.0, 0),
        ]);
    }

    #[test]
    fn desktop_and_apps_toggles_back() {
        let (out, kbd, _) = outputs();
        let mut da = make_scroll(Some(&"desktop-and-apps".into()), &out, &scroller(&out)).unwrap();
        da.begin();
        for v in [120, 120, -120, -120] {
            da.wheel(v);
        }
        let letters: Vec<u16> = pressed_keys(&kbd).into_iter()
            .filter(|&(c, v)| v == 1 && c != KeyCode::KEY_LEFTMETA.0).map(|e| e.0).collect();
        assert_eq!(letters, [KeyCode::KEY_A.0, KeyCode::KEY_A.0, KeyCode::KEY_D.0],
                   "grid, close grid, desktop");
    }

    #[test]
    fn zoom_holds_ctrl_around_raw_wheel() {
        let (out, kbd, mouse) = outputs();
        let mut z = make_scroll(Some(&"zoom".into()), &out, &scroller(&out)).unwrap();
        z.begin();
        z.wheel(120);
        z.end();
        assert_eq!(pressed_keys(&kbd), [(KeyCode::KEY_LEFTCTRL.0, 1), (KeyCode::KEY_LEFTCTRL.0, 0)]);
        assert!(mouse.borrow().events().contains(&(crate::io::EV_REL, evdev::RelativeAxisCode::REL_WHEEL.0, 1)));
    }

    #[test]
    fn default_config_builds() {
        let (out, _, _) = outputs();
        let bindings = build_bindings(&config::defaults(), &out, &scroller(&out));
        assert_eq!(bindings.len(), 3);
        assert!(bindings[&FRONT].double.is_some());
        assert!(bindings[&BACK].drag.is_some());
    }
}
