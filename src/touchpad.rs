//! Virtual touchpads: multi-finger swipes driven by mouse drags, and two-finger
//! scrolling driven by the wheel.
//!
//! libinput discards touches that move too far too fast ("kernel bug: touch jump"),
//! so both send frames at a real touchpad's pace (~8 ms) and in bounded steps.

use std::cell::RefCell;
use std::rc::Rc;

use evdev::{AbsoluteAxisCode as Abs, KeyCode};

use crate::io::{EV_ABS, EV_KEY, SharedClock, Sink};

pub const RES: i32 = 40; // units per mm
pub const WIDTH: i32 = 12000; // 300 x 200 mm, room for long drags
pub const HEIGHT: i32 = 8000;
const FRAME: f64 = 0.008; // s between frames
/// A short touch that barely moves would count as a tap (= a click), so touches last
/// at least this long, past libinput's tap timeout.
const TAP_GUARD: f64 = 0.25;
const SPACING: i32 = 500; // 12.5 mm between fingers

pub struct VirtualTouchpad {
    pub sink: Box<dyn Sink>,
    next_id: i32,
}

pub type SharedTouchpad = Rc<RefCell<VirtualTouchpad>>;

impl VirtualTouchpad {
    pub fn new(sink: Box<dyn Sink>) -> SharedTouchpad {
        Rc::new(RefCell::new(VirtualTouchpad { sink, next_id: 0 }))
    }

    fn tool(fingers: usize) -> KeyCode {
        if fingers == 2 { KeyCode::BTN_TOOL_DOUBLETAP } else { KeyCode::BTN_TOOL_TRIPLETAP }
    }

    /// Writes finger positions around (cx, cy); with `touch_down` they are new touches.
    fn write_fingers(&mut self, fingers: usize, cx: i32, cy: i32, touch_down: bool) {
        for i in 0..fingers {
            self.sink.write(EV_ABS, Abs::ABS_MT_SLOT.0, i as i32);
            if touch_down {
                self.sink.write(EV_ABS, Abs::ABS_MT_TRACKING_ID.0, self.next_id + i as i32);
            }
            let offset = (i as f64 - (fingers as f64 - 1.0) / 2.0) * SPACING as f64;
            self.sink.write(EV_ABS, Abs::ABS_MT_POSITION_X.0, cx + offset.round() as i32);
            self.sink.write(EV_ABS, Abs::ABS_MT_POSITION_Y.0, cy);
        }
        self.sink.write(EV_ABS, Abs::ABS_X.0, cx);
        self.sink.write(EV_ABS, Abs::ABS_Y.0, cy);
        if touch_down {
            self.next_id = (self.next_id + fingers as i32) % 60000;
            self.sink.write(EV_KEY, KeyCode::BTN_TOUCH.0, 1);
            self.sink.write(EV_KEY, Self::tool(fingers).0, 1);
        }
        self.sink.syn();
    }

    fn lift(&mut self, fingers: usize) {
        for i in 0..fingers {
            self.sink.write(EV_ABS, Abs::ABS_MT_SLOT.0, i as i32);
            self.sink.write(EV_ABS, Abs::ABS_MT_TRACKING_ID.0, -1);
        }
        self.sink.write(EV_KEY, KeyCode::BTN_TOUCH.0, 0);
        self.sink.write(EV_KEY, Self::tool(fingers).0, 0);
        self.sink.syn();
    }
}

fn clamp(v: f64, limit: f64) -> f64 {
    v.max(-limit).min(limit)
}

/// What a held button does when the mouse moves.
pub trait Drag {
    fn begin(&mut self);
    fn motion(&mut self, dx: i32, dy: i32);
    /// Called at the end of every mouse event frame.
    fn frame(&mut self);
    fn end(&mut self);
}

/// A two- or three-finger swipe that follows the mouse.
pub struct TouchpadDrag {
    pad: SharedTouchpad,
    clock: SharedClock,
    fingers: usize,
    sx: f64, // touchpad units per mouse count, signed
    sy: f64,
    max_x: f64,
    max_y: f64,
    x: f64,
    y: f64,
    pending_x: f64,
    pending_y: f64,
    started: f64,
    last_frame: f64,
    moved: f64,
}

const DRAG_MAX_STEP: f64 = 120.0; // units (3 mm) per axis per frame

impl TouchpadDrag {
    pub fn new(pad: SharedTouchpad, clock: SharedClock, fingers: usize, sx: f64, sy: f64) -> Self {
        let margin = 200.0;
        TouchpadDrag {
            pad,
            clock,
            fingers,
            sx,
            sy,
            max_x: (WIDTH / 2) as f64 - (fingers as f64 - 1.0) * SPACING as f64 - margin,
            max_y: (HEIGHT / 2) as f64 - margin,
            x: 0.0,
            y: 0.0,
            pending_x: 0.0,
            pending_y: 0.0,
            started: 0.0,
            last_frame: 0.0,
            moved: 0.0,
        }
    }

    fn write(&self, touch_down: bool) {
        let cx = WIDTH / 2 + self.x.round() as i32;
        let cy = HEIGHT / 2 + self.y.round() as i32;
        self.pad.borrow_mut().write_fingers(self.fingers, cx, cy, touch_down);
    }
}

impl Drag for TouchpadDrag {
    fn begin(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
        self.pending_x = 0.0;
        self.pending_y = 0.0;
        self.moved = 0.0;
        self.started = self.clock.now();
        self.last_frame = self.started;
        self.write(true);
    }

    fn motion(&mut self, dx: i32, dy: i32) {
        self.pending_x += dx as f64 * self.sx;
        self.pending_y += dy as f64 * self.sy;
    }

    fn frame(&mut self) {
        let now = self.clock.now();
        if now - self.last_frame < FRAME {
            return;
        }
        let sx = clamp(self.pending_x, DRAG_MAX_STEP);
        let sy = clamp(self.pending_y, DRAG_MAX_STEP);
        self.pending_x -= sx;
        self.pending_y -= sy;
        let x = clamp(self.x + sx, self.max_x);
        let y = clamp(self.y + sy, self.max_y);
        if x.round() == self.x.round() && y.round() == self.y.round() {
            return;
        }
        self.moved += (x - self.x).abs() + (y - self.y).abs();
        self.x = x;
        self.y = y;
        self.last_frame = now;
        self.write(false);
    }

    fn end(&mut self) {
        // Play out movement still waiting for a frame, so a flick keeps its speed.
        for _ in 0..10 {
            if self.pending_x.abs() < 1.0 && self.pending_y.abs() < 1.0 {
                break;
            }
            self.clock.sleep(FRAME);
            self.frame();
        }
        if self.moved < (3 * RES) as f64 {
            self.clock.sleep(TAP_GUARD - (self.clock.now() - self.started));
        }
        self.pad.borrow_mut().lift(self.fingers);
    }
}

/// Two-finger scrolling fed by the SmoothScroller. Fingers go down on the first
/// notch, move at touchpad frame rate and lift once the wheel has been idle a moment.
pub struct TouchpadScroll {
    pad: SharedTouchpad,
    clock: SharedClock,
    sign: f64,
    down: bool,
    y: f64,
    pending: f64,
    down_since: f64,
    last_frame: f64,
}

pub const NOTCH: f64 = 100.0; // touchpad units (2.5 mm) per wheel notch at medium speed
const SCROLL_MAX_STEP: f64 = 160.0;
const LIFT_DELAY: f64 = 0.3; // s of wheel idle before the fingers lift
const LIMIT: f64 = 3000.0; // re-place the fingers before they run off the pad

impl TouchpadScroll {
    pub fn new(pad: SharedTouchpad, clock: SharedClock, reverse: bool) -> Self {
        // With matching natural-scroll settings for mouse and touchpad, wheel up (+)
        // matches the fingers moving up (-y).
        TouchpadScroll {
            pad,
            clock,
            sign: if reverse { 1.0 } else { -1.0 },
            down: false,
            y: 0.0,
            pending: 0.0,
            down_since: 0.0,
            last_frame: 0.0,
        }
    }

    pub fn emit(&mut self, v120: i32) {
        self.pending += v120 as f64 / 120.0 * NOTCH * self.sign;
    }

    pub fn stop(&mut self) {
        self.pending = 0.0;
        if self.down {
            self.lift();
        }
    }

    pub fn timeout(&self) -> Option<f64> {
        let now = self.clock.now();
        if self.pending.abs() >= 1.0 {
            return Some(if self.down { (self.last_frame + FRAME - now).max(0.0) } else { 0.0 });
        }
        if self.down {
            let due = (self.last_frame + LIFT_DELAY).max(self.down_since + TAP_GUARD);
            return Some((due - now).max(0.0));
        }
        None
    }

    pub fn tick(&mut self) {
        if self.timeout() != Some(0.0) {
            return;
        }
        let now = self.clock.now();
        if self.pending.abs() < 1.0 {
            self.lift();
        } else if !self.down {
            self.place(); // movement starts on the next frame
        } else if self.y.abs() > LIMIT && now - self.down_since >= TAP_GUARD {
            self.lift();
        } else {
            let step = clamp(self.pending, SCROLL_MAX_STEP);
            self.pending -= step;
            self.y += step;
            self.write(false);
            self.last_frame = now;
        }
    }

    fn write(&self, touch_down: bool) {
        let cy = HEIGHT / 2 + self.y.round() as i32;
        self.pad.borrow_mut().write_fingers(2, WIDTH / 2, cy, touch_down);
    }

    fn place(&mut self) {
        self.y = 0.0;
        self.write(true);
        self.down = true;
        self.down_since = self.clock.now();
        self.last_frame = self.down_since;
    }

    fn lift(&mut self) {
        self.pad.borrow_mut().lift(2);
        self.down = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Clock;
    use crate::io::testing::{FakeClock, Recorder};

    struct Shared(Rc<RefCell<Recorder>>);
    impl Sink for Shared {
        fn write(&mut self, t: u16, c: u16, v: i32) {
            self.0.borrow_mut().write(t, c, v)
        }
        fn syn(&mut self) {
            self.0.borrow_mut().syn()
        }
    }

    fn setup() -> (Rc<RefCell<Recorder>>, SharedTouchpad, Rc<FakeClock>) {
        let rec = Recorder::shared();
        let pad = VirtualTouchpad::new(Box::new(Shared(rec.clone())));
        (rec, pad, FakeClock::new(10.0))
    }

    /// (y of ABS_Y, touch state) after each frame.
    fn track(frames: &[Vec<(u16, u16, i32)>]) -> Vec<(i32, i32)> {
        let (mut y, mut touch) = (0, 0);
        frames.iter().map(|f| {
            for &(t, c, v) in f {
                if t == EV_ABS && c == Abs::ABS_Y.0 { y = v }
                if t == EV_KEY && c == KeyCode::BTN_TOUCH.0 { touch = v }
            }
            (y, touch)
        }).collect()
    }

    #[test]
    fn drag_frames_are_paced_and_bounded() {
        let (rec, pad, clock) = setup();
        let mut drag = TouchpadDrag::new(pad, clock.clone(), 3, 1.5, 1.5);
        drag.begin();
        let mut frame_times = vec![];
        for _ in 0..300 {
            drag.motion(-4, 0);
            let n = rec.borrow().frames.len();
            drag.frame();
            if rec.borrow().frames.len() > n { frame_times.push(clock.now()) }
            clock.advance(0.001);
        }
        drag.end();
        let gaps: Vec<f64> = frame_times.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g >= FRAME - 1e-9), "{gaps:?}");
        let frames = &rec.borrow().frames;
        assert_eq!(track(frames).last().unwrap().1, 0, "fingers lifted");
        let xs: Vec<i32> = frames.iter().flatten()
            .filter(|&&(t, c, _)| t == EV_ABS && c == Abs::ABS_X.0).map(|e| e.2).collect();
        assert!(xs.windows(2).all(|w| (w[1] - w[0]).abs() <= DRAG_MAX_STEP as i32));
        assert!(xs[0] - xs.last().unwrap() > 1500, "moved left: {xs:?}");
    }

    #[test]
    fn short_still_drag_outlasts_tap_timeout() {
        let (rec, pad, clock) = setup();
        let mut drag = TouchpadDrag::new(pad, clock.clone(), 2, 1.0, 1.0);
        drag.begin();
        drag.end();
        assert!(clock.now() - 10.0 >= TAP_GUARD - 1e-9);
        assert_eq!(rec.borrow().frames.len(), 2);
    }

    fn run_until_idle(ts: &mut TouchpadScroll, clock: &FakeClock) {
        while let Some(t) = ts.timeout() {
            clock.advance(t.max(0.0005));
            ts.tick();
        }
    }

    #[test]
    fn one_notch_scrolls_one_notch_then_lifts() {
        let (rec, pad, clock) = setup();
        let mut ts = TouchpadScroll::new(pad, clock.clone(), false);
        ts.emit(120);
        run_until_idle(&mut ts, &clock);
        let t = track(&rec.borrow().frames);
        assert_eq!(t[0], (HEIGHT / 2, 1));
        assert_eq!(t.last().unwrap().1, 0);
        assert_eq!(t[0].0 - t[t.len() - 2].0, NOTCH as i32, "wheel up moves fingers up");
        assert!(clock.now() - 10.0 >= TAP_GUARD);
    }

    #[test]
    fn long_spin_replaces_fingers_and_never_jumps() {
        let (rec, pad, clock) = setup();
        let mut ts = TouchpadScroll::new(pad, clock.clone(), true);
        for _ in 0..80 {
            ts.emit(-120);
            clock.advance(0.01);
            ts.tick();
        }
        run_until_idle(&mut ts, &clock);
        let t = track(&rec.borrow().frames);
        let touches = t.windows(2).filter(|w| w[0].1 == 0 && w[1].1 == 1).count() + 1;
        assert!(touches >= 2, "fingers re-placed");
        let steps = t.windows(2).filter(|w| w[0].1 == 1 && w[1].1 == 1)
            .map(|w| (w[1].0 - w[0].0).abs()).max().unwrap();
        assert!(steps <= SCROLL_MAX_STEP as i32);
        assert!(t.iter().all(|&(y, _)| (y - HEIGHT / 2).abs() <= LIMIT as i32 + SCROLL_MAX_STEP as i32));
    }

    #[test]
    fn stop_lifts_at_once() {
        let (rec, pad, clock) = setup();
        let mut ts = TouchpadScroll::new(pad, clock.clone(), false);
        ts.emit(120);
        ts.tick();
        assert!(ts.down);
        ts.stop();
        assert_eq!(track(&rec.borrow().frames).last().unwrap().1, 0);
        assert_eq!(ts.timeout(), None);
    }
}
