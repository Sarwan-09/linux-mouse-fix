//! Event output and time, behind traits so the logic can be tested without devices.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use evdev::InputEvent;
use evdev::uinput::VirtualDevice;

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;

/// Somewhere events go: a virtual device, or a recorder in tests.
pub trait Sink {
    fn write(&mut self, type_: u16, code: u16, value: i32);
    /// Ends the frame (SYN_REPORT).
    fn syn(&mut self);
}

pub type SharedSink = Rc<RefCell<dyn Sink>>;

pub trait Clock {
    /// Seconds on a monotonic clock.
    fn now(&self) -> f64;
    fn sleep(&self, seconds: f64);
}

pub type SharedClock = Rc<dyn Clock>;

pub struct SystemClock(Instant);

impl SystemClock {
    pub fn new() -> Self {
        SystemClock(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }

    fn sleep(&self, seconds: f64) {
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
    }
}

/// A uinput device; events are buffered until syn() sends them as one frame.
pub struct UinputSink {
    dev: VirtualDevice,
    buf: Vec<InputEvent>,
}

impl UinputSink {
    pub fn new(dev: VirtualDevice) -> Self {
        UinputSink { dev, buf: Vec::new() }
    }
}

impl Sink for UinputSink {
    fn write(&mut self, type_: u16, code: u16, value: i32) {
        self.buf.push(InputEvent::new(type_, code, value));
    }

    fn syn(&mut self) {
        // emit() appends the SYN_REPORT itself.
        if let Err(err) = self.dev.emit(&self.buf) {
            eprintln!("WARNING could not send input events: {err}");
        }
        self.buf.clear();
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::cell::Cell;

    /// A clock that only moves when told to (sleep() moves it too).
    pub struct FakeClock(pub Cell<f64>);

    impl FakeClock {
        pub fn new(t: f64) -> Rc<Self> {
            Rc::new(FakeClock(Cell::new(t)))
        }
        pub fn set(&self, t: f64) {
            self.0.set(t)
        }
        pub fn advance(&self, dt: f64) {
            self.0.set(self.0.get() + dt)
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> f64 {
            self.0.get()
        }
        fn sleep(&self, seconds: f64) {
            self.advance(seconds.max(0.0))
        }
    }

    /// Records frames: each syn() closes the events written since the last one.
    #[derive(Default)]
    pub struct Recorder {
        pub pending: Vec<(u16, u16, i32)>,
        pub frames: Vec<Vec<(u16, u16, i32)>>,
    }

    impl Sink for Recorder {
        fn write(&mut self, type_: u16, code: u16, value: i32) {
            self.pending.push((type_, code, value));
        }
        fn syn(&mut self) {
            self.frames.push(std::mem::take(&mut self.pending));
        }
    }

    impl Recorder {
        pub fn shared() -> Rc<RefCell<Recorder>> {
            Rc::new(RefCell::new(Recorder::default()))
        }
        /// All events, frame boundaries dropped.
        pub fn events(&self) -> Vec<(u16, u16, i32)> {
            self.frames.iter().flatten().copied().chain(self.pending.iter().copied()).collect()
        }
    }
}
