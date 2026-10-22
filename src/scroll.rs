//! Turns wheel notches into accelerated, eased streams of scroll movement.

use evdev::RelativeAxisCode as Rel;

use crate::io::{EV_REL, SharedClock, SharedSink};

pub const FRAME: f64 = 1.0 / 120.0;
const STREAK_GAP: f64 = 0.15; // s between notches that count as a fast spin

/// Time constant of the ease-out, by smoothness setting; None = no smoothing.
pub fn tau(smooth: &str) -> Option<f64> {
    match smooth {
        "regular" => Some(0.06),
        "high" => Some(0.13),
        _ => None,
    }
}

pub fn speed(name: &str) -> f64 {
    match name {
        "low" => 0.7,
        "high" => 1.5,
        _ => 1.0,
    }
}

pub struct SmoothScroller {
    emit: Box<dyn FnMut(i32)>, // one frame of vertical movement, in 1/120 notches
    on_stop: Option<Box<dyn FnMut()>>,
    clock: SharedClock,
    tau: Option<f64>,
    speed: f64,
    acceleration: bool,
    remaining: f64,
    carry: f64,
    streak: f64, // notches spun in quick succession
    direction: i32,
    last_notch: f64,
    last_frame: f64,
}

impl SmoothScroller {
    pub fn new(emit: Box<dyn FnMut(i32)>, on_stop: Option<Box<dyn FnMut()>>, clock: SharedClock,
               smooth: &str, speed_name: &str, acceleration: bool) -> Self {
        SmoothScroller {
            emit,
            on_stop,
            clock,
            tau: tau(smooth),
            speed: speed(speed_name),
            acceleration,
            remaining: 0.0,
            carry: 0.0,
            streak: 0.0,
            direction: 0,
            last_notch: f64::NEG_INFINITY,
            last_frame: 0.0,
        }
    }

    pub fn stop(&mut self) {
        self.remaining = 0.0;
        if let Some(on_stop) = &mut self.on_stop {
            on_stop();
        }
    }

    /// A wheel movement; `factor` overrides speed and acceleration.
    pub fn wheel(&mut self, v120: i32, factor: Option<f64>) {
        let now = self.clock.now();
        let direction = if v120 > 0 { 1 } else { -1 };
        if direction != self.direction {
            self.remaining = 0.0; // reversing stops the glide at once
            self.streak = 0.0;
        } else if now - self.last_notch < STREAK_GAP {
            self.streak = (self.streak + v120.abs() as f64 / 120.0).min(10.0);
        } else {
            self.streak = 0.0;
        }
        self.direction = direction;
        self.last_notch = now;
        let factor = factor.unwrap_or_else(|| {
            self.speed * if self.acceleration { 1.0 + 0.3 * self.streak } else { 1.0 }
        });
        if self.tau.is_none() {
            self.out(v120 as f64 * factor);
            return;
        }
        if self.remaining.abs() < 0.5 {
            self.last_frame = now;
        }
        self.remaining += v120 as f64 * factor;
    }

    pub fn timeout(&self) -> Option<f64> {
        if self.tau.is_none() || self.remaining.abs() < 0.5 {
            return None;
        }
        Some((self.last_frame + FRAME - self.clock.now()).max(0.0))
    }

    pub fn tick(&mut self) {
        let (Some(tau), Some(0.0)) = (self.tau, self.timeout()) else { return };
        let now = self.clock.now();
        let mut portion = self.remaining * (1.0 - (-(now - self.last_frame) / tau).exp());
        if (self.remaining - portion).abs() < 1.0 {
            portion = self.remaining;
        }
        self.remaining -= portion;
        self.last_frame = now;
        self.out(portion);
    }

    fn out(&mut self, amount: f64) {
        self.carry += amount;
        let whole = self.carry.trunc();
        self.carry -= whole;
        if whole != 0.0 {
            (self.emit)(whole as i32);
        }
    }
}

/// Emits vertical wheel movement on a virtual mouse: hi-res units, plus a legacy
/// REL_WHEEL step every full notch.
pub fn hires_emitter(mouse: SharedSink) -> Box<dyn FnMut(i32)> {
    let mut acc = 0;
    Box::new(move |v120| {
        let mut m = mouse.borrow_mut();
        m.write(EV_REL, Rel::REL_WHEEL_HI_RES.0, v120);
        acc += v120;
        while acc.abs() >= 120 {
            let step = acc.signum();
            m.write(EV_REL, Rel::REL_WHEEL.0, step);
            acc -= step * 120;
        }
        m.syn();
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Clock;
    use crate::io::testing::{FakeClock, Recorder};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn scroller(smooth: &str, accel: bool, clock: Rc<FakeClock>) -> (SmoothScroller, Rc<RefCell<Vec<(f64, i32)>>>) {
        let out = Rc::new(RefCell::new(vec![]));
        let (o, c) = (out.clone(), clock.clone());
        let emit = Box::new(move |v| o.borrow_mut().push((c.now(), v)));
        (SmoothScroller::new(emit, None, clock, smooth, "medium", accel), out)
    }

    fn drain(s: &mut SmoothScroller, clock: &FakeClock) {
        while let Some(t) = s.timeout() {
            clock.advance(t.max(1e-4));
            s.tick();
        }
    }

    #[test]
    fn one_notch_glides() {
        let clock = FakeClock::new(100.0);
        let (mut s, out) = scroller("high", true, clock.clone());
        s.wheel(120, None);
        drain(&mut s, &clock);
        let out = out.borrow();
        assert_eq!(out.iter().map(|e| e.1).sum::<i32>(), 120);
        let duration = out.last().unwrap().0 - 100.0;
        assert!(out.len() > 10 && duration > 0.2 && duration < 1.0, "{} frames, {duration}s", out.len());
    }

    #[test]
    fn fast_spin_accelerates() {
        let clock = FakeClock::new(200.0);
        let (mut s, out) = scroller("high", true, clock.clone());
        for _ in 0..5 {
            s.wheel(120, None);
            clock.advance(0.05);
        }
        drain(&mut s, &clock);
        assert!(out.borrow().iter().map(|e| e.1).sum::<i32>() > 600);
    }

    #[test]
    fn reversing_stops_the_glide() {
        let clock = FakeClock::new(300.0);
        let (mut s, out) = scroller("high", true, clock.clone());
        s.wheel(120, None);
        clock.advance(0.02);
        s.tick();
        s.wheel(-120, None);
        drain(&mut s, &clock);
        let out = out.borrow();
        assert!(out.iter().map(|e| e.1).sum::<i32>() < 0);
        assert!(out.last().unwrap().1 < 0);
    }

    #[test]
    fn off_is_immediate() {
        let clock = FakeClock::new(0.0);
        let (mut s, out) = scroller("off", false, clock);
        s.wheel(120, None);
        assert_eq!(out.borrow().iter().map(|e| e.1).collect::<Vec<_>>(), vec![120]);
        assert_eq!(s.timeout(), None);
    }

    #[test]
    fn hires_emitter_sends_legacy_notches() {
        let rec = Recorder::shared();
        let mut emit = hires_emitter(rec.clone());
        for v in [60, 50, 30, -20] {
            emit(v);
        }
        let legacy: Vec<_> = rec.borrow().events().into_iter()
            .filter(|&(_, c, _)| c == Rel::REL_WHEEL.0).collect();
        assert_eq!(legacy, vec![(EV_REL, Rel::REL_WHEEL.0, 1)]);
    }
}
