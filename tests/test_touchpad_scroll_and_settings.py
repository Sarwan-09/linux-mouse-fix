"""Run from the repository root: python3 -I tests/test_touchpad_scroll_and_settings.py"""
import importlib.machinery, importlib.util, sys, os, time
import os, tempfile
from evdev import ecodes as e
loader = importlib.machinery.SourceFileLoader("lmf", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "linux-mouse-fix"))
spec = importlib.util.spec_from_loader("lmf", loader); m = importlib.util.module_from_spec(spec); loader.exec_module(m)
now = [0.0]; clock = lambda: now[0]
frames = []; cur = {}
class FakePad:
    def __init__(self, *a, **k): pass
    def write(self, t, c, v):
        if c == e.ABS_Y: cur['y'] = v
        if c == e.BTN_TOUCH: cur['touch'] = v
        if c == e.ABS_MT_TRACKING_ID: cur.setdefault('ids', []).append(v)
    def syn(self): frames.append((now[0], dict(cur))); cur.pop('ids', None)
m.evdev.UInput = FakePad
pad = m.VirtualTouchpad("scroll touchpad")
ts = m.TouchpadScroll(pad, reverse=False, clock=clock)
sc = m.SmoothScroller(ts.emit, "high", "medium", True, clock, on_stop=ts.stop)
def run_until_idle(limit=5.0):
    end = now[0] + limit
    while now[0] < end:
        ts_ = [t for t in (sc.timeout(), ts.timeout()) if t is not None]
        if not ts_: return
        now[0] += max(min(ts_), 0.0005)
        sc.tick(); ts.tick()
now[0] = 10.0; sc.wheel(120)          # one notch wheel up
run_until_idle()
assert frames[0][1]['touch'] == 1 and frames[-1][1]['touch'] == 0, (frames[0], frames[-1])
ys = [f[1]['y'] for f in frames]
moves = frames[1:-1]
gaps = [b[0] - a[0] for a, b in zip(frames[:-1], frames[1:-1])]
print("1 notch: %d frames, y %d -> %d, min gap %.1f ms, touch lasted %.0f ms" % (
      len(frames), ys[0], ys[-1], min(gaps) * 1000, (frames[-1][0] - frames[0][0]) * 1000))
assert 95 <= ys[0] - ys[-1] <= 100, ys[-1]       # wheel up -> fingers up by ~NOTCH
assert min(gaps) >= 0.008 - 1e-9
assert frames[-1][0] - frames[0][0] >= ts.TAP_GUARD
# long fast spin re-places fingers before the edge, never moves > MAX_STEP per frame
frames.clear(); now[0] = 20.0
for i in range(60):
    sc.wheel(-120); now[0] += 0.03; sc.tick(); ts.tick()
run_until_idle(10)
touches = [f for f in frames if f[1].get('ids') and f[1]['ids'][0] != -1]
steps = [abs(b[1]['y'] - a[1]['y']) for a, b in zip(frames, frames[1:]) if a[1]['touch'] and b[1]['touch'] and not b[1].get('ids')]
print("60 fast notches: %d touches, max step %d, final touch %d" % (len(touches), max(steps), frames[-1][1]['touch']))
assert max(steps) <= ts.MAX_STEP and frames[-1][1]['touch'] == 0 and len(touches) >= 2
# stop() lifts immediately
frames.clear(); sc.wheel(120); now[0] += 0.01; sc.tick(); ts.tick(); assert ts.down; sc.stop()
assert frames[-1][1]['touch'] == 0 and ts.timeout() is None
print("touchpad scroll ok")

# settings reload
cfg = os.path.join(tempfile.mkdtemp(), "cfg.toml")
st = m.Settings(cfg)
if os.path.exists(cfg): os.remove(cfg)
assert st.reload() is False and st.config["enabled"]
open(cfg, "w").write('enabled = false\n[scroll]\nspeed = "high"\n')
assert st.reload() and st.config["enabled"] is False and st.config["scroll"]["speed"] == "high" and st.config["scroll"]["smooth"] == "high"
assert st.reload() is False
time.sleep(0.01); open(cfg, "w").write('[scroll]\nspeed = "warp"\n')
assert st.reload() is False and st.config["scroll"]["speed"] == "high"   # bad file ignored
time.sleep(0.01); open(cfg, "w").write('[buttons.back]\nhold = "ctrl+shift+t"\n')
assert st.reload() and st.config["buttons"]["back"]["hold"] == "ctrl+shift+t" and st.config["buttons"]["back"]["click"] == "back"
print("settings ok")
