"""Run from the repository root: python3 -I tests/test_buttons_and_scrolling.py"""
import importlib.machinery, importlib.util, sys, time
import os
from evdev import ecodes as e
loader = importlib.machinery.SourceFileLoader("lmf", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "linux-mouse-fix"))
spec = importlib.util.spec_from_loader("lmf", loader); m = importlib.util.module_from_spec(spec); loader.exec_module(m)
out = []; now = [0.0]; clock = lambda: now[0]
class FakeDrag:
    def begin(self): out.append("drag-begin")
    def move(self, dx, dy): pass
    def frame(self): out.append("frame")
    def end(self): out.append("drag-end")
class FakeScroll:
    def begin(self): out.append("scroll-begin")
    def wheel(self, v): out.append(("wheel", v))
    def end(self): out.append("scroll-end")
A = lambda name: (lambda: out.append(name))
B = {e.BTN_MIDDLE: m.Binding(click=A("mclick"), hold=A("hold"), drag=FakeDrag(), scroll=FakeScroll()),
     e.BTN_EXTRA: m.Binding(click=A("fclick"), double=A("double"), drag=FakeDrag()),
     e.BTN_SIDE: m.Binding(click=A("back"))}
b = m.Buttons(B, clock)
def tap(c): b.button(c, 1); b.button(c, 0)
# middle: quick click
tap(e.BTN_MIDDLE); assert out == ["mclick"], out; out.clear()
# middle hold
now[0] = 1; b.button(e.BTN_MIDDLE, 1); assert abs(b.timeout() - m.HOLD_TIME) < 1e-9
now[0] = 1.5; b.tick(); b.button(e.BTN_MIDDLE, 0); assert out == ["hold"], out; out.clear()
# middle drag: no hold, no click
now[0] = 2; b.button(e.BTN_MIDDLE, 1); assert b.motion(20, 0); now[0] = 3; b.tick(); b.button(e.BTN_MIDDLE, 0)
assert out == ["drag-begin", "drag-end"], out; out.clear()
# middle + scroll
b.button(e.BTN_MIDDLE, 1); assert b.wheel(120); assert b.wheel(-120); b.button(e.BTN_MIDDLE, 0)
assert out == ["scroll-begin", ("wheel", 120), ("wheel", -120), "scroll-end"], out; out.clear()
# wheel without held button not consumed; back button with no drag lets pointer move, still clicks
assert not b.wheel(120)
b.button(e.BTN_SIDE, 1); assert not b.motion(50, 0); assert not b.wheel(120); b.button(e.BTN_SIDE, 0)
assert out == ["back"], out; out.clear()
# front: single (delayed), double
now[0] = 10; tap(e.BTN_EXTRA); b.tick(); assert out == []; now[0] = 10.31; b.tick(); assert out == ["fclick"]; out.clear()
now[0] = 11; tap(e.BTN_EXTRA); now[0] = 11.1; tap(e.BTN_EXTRA); now[0] = 12; b.tick(); assert out == ["double"], out; out.clear()
# front tap then middle press: pending front click fires immediately
now[0] = 13; tap(e.BTN_EXTRA); now[0] = 13.1; tap(e.BTN_MIDDLE); assert out == ["fclick", "mclick"], out; out.clear()
# other button pressed while one active is swallowed
b.button(e.BTN_MIDDLE, 1); tap(e.BTN_SIDE); b.button(e.BTN_MIDDLE, 0); assert out == ["mclick"], out; out.clear()
# unbound button not consumed
assert not b.button(e.BTN_LEFT, 1)
print("buttons ok")

# smooth scroller: totals preserved, spread over time, legacy wheel emitted
emitted = []
sc = m.SmoothScroller(lambda v: emitted.append((now[0], v)), "high", "medium", True, clock)
now[0] = 100.0; sc.wheel(120)
t = now[0]
while sc.timeout() is not None:
    now[0] += sc.timeout() or 0; sc.tick()
assert sum(v for _, v in emitted) == 120, sum(v for _, v in emitted)
dur = emitted[-1][0] - t
print("one notch: %d frames over %.0f ms, first %d" % (len(emitted), dur * 1000, emitted[0][1]))
assert len(emitted) > 10 and 0.2 < dur < 1.0
# acceleration: 5 quick notches scroll further than 5 * 120
emitted.clear(); now[0] = 200.0
for i in range(5):
    sc.wheel(120); now[0] += 0.05
while sc.timeout() is not None:
    now[0] += sc.timeout() or 0; sc.tick()
fast = sum(v for _, v in emitted); print("5 fast notches ->", fast); assert fast > 600
# reversing stops glide
emitted.clear(); now[0] = 300; sc.wheel(120); now[0] += 0.02; sc.tick(); sc.wheel(-120)
while sc.timeout() is not None:
    now[0] += sc.timeout() or 0; sc.tick()
assert sum(v for _, v in emitted) < 0 + 120 and emitted[-1][1] < 0
# smoothing off: immediate
emitted.clear(); off = m.SmoothScroller(lambda v: emitted.append(v), "off", "medium", False, clock)
off.wheel(120); assert emitted == [120] and off.timeout() is None
print("scroller ok")

# hires emitter writes legacy notches
class FakeMouse:
    def __init__(self): self.ev = []
    def write(self, t, c, v): self.ev.append((c, v))
    def syn(self): self.ev.append("syn")
fm = FakeMouse(); em = m.hires_emitter(fm)
for v in (60, 50, 30, -20): em(v)
assert [x for x in fm.ev if x != "syn" and x[0] == e.REL_WHEEL] == [(e.REL_WHEEL, 1)], fm.ev

# step scroll / desktop-and-apps / app switcher with fake keyboard
class FakeKbd:
    def __init__(self): self.ev = []
    def write(self, t, c, v): self.ev.append((c, v))
    def syn(self): pass
m.time.sleep = lambda s: None
kb = FakeKbd(); sw = m.make_scroll("app-switcher", kb, fm, sc)
sw.begin(); sw.wheel(60); assert kb.ev == []  # half a notch: Super not pressed yet
sw.wheel(60); sw.wheel(-120); sw.end()
names = [(e.KEY[c] if not isinstance(e.KEY[c], list) else e.KEY[c][0], v) for c, v in kb.ev]
print("app-switcher:", names)
assert names[0] == ("KEY_LEFTMETA", 1) and names[-1] == ("KEY_LEFTMETA", 0)
kb.ev.clear(); da = m.make_scroll("desktop-and-apps", kb, fm, sc)
da.begin(); da.wheel(120); da.wheel(120); da.wheel(-120); da.wheel(-120)
keys = [c for c, v in kb.ev if v == 1 and c != e.KEY_LEFTMETA]
assert keys == [e.KEY_A, e.KEY_A, e.KEY_D], keys   # grid, close grid, desktop
print("scroll effects ok")

# routing: wheel goes to scroller, legacy REL_WHEEL dropped for hi-res mice, syn forwarded
class Ev:
    def __init__(s, t, c, v): s.type, s.code, s.value = t, c, v
fm2 = FakeMouse(); got = []
class Sc:
    def wheel(s, v): got.append(v)
bb = m.Buttons({}, clock)
for ev in (Ev(e.EV_REL, e.REL_WHEEL, 1), Ev(e.EV_REL, e.REL_WHEEL_HI_RES, 120), Ev(e.EV_REL, e.REL_X, 5), Ev(e.EV_SYN, 0, 0)):
    m.handle(ev, bb, Sc(), fm2, True)
assert got == [120] and fm2.ev == [(e.REL_X, 5), (0, 0)], (got, fm2.ev)
got.clear(); m.handle(Ev(e.EV_REL, e.REL_WHEEL, -1), bb, Sc(), fm2, False); assert got == [-120]
print("routing ok")
