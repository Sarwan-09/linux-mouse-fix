"""Run from the repository root: python3 -I tests/test_drag.py"""
import importlib.machinery, importlib.util, sys, time
import os
from evdev import ecodes as e
loader = importlib.machinery.SourceFileLoader("lmf", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "linux-mouse-fix"))
spec = importlib.util.spec_from_loader("lmf", loader); m = importlib.util.module_from_spec(spec); loader.exec_module(m)
frames = []; cur = {}
class FakePad:
    def __init__(self, *a, **k): pass
    def write(self, t, c, v):
        if c == e.ABS_MT_SLOT: cur['slot'] = v
        if c == e.ABS_MT_POSITION_X: cur['x%d' % cur['slot']] = v
        if c == e.ABS_Y: cur['y'] = v
        if c in (e.BTN_TOOL_DOUBLETAP, e.BTN_TOOL_TRIPLETAP): cur['tool'] = (c, v)
    def syn(self): frames.append((time.monotonic(), dict(cur)))
m.evdev.UInput = FakePad
tp = m.VirtualTouchpad()
for name, fingers in (("spaces", 3), ("navigate", 2)):
    frames.clear(); cur.clear()
    d = m.make_drag(name, tp, 1.5, True)
    b = m.Buttons({e.BTN_EXTRA: m.Binding(drag=d)})
    b.button(e.BTN_EXTRA, 1)
    for i in range(300):
        b.motion(-4, 0); b.frame(); time.sleep(0.001)
    b.button(e.BTN_EXTRA, 0)
    gaps = [y[0]-x[0] for x, y in zip(frames[1:-1], frames[2:-1])]
    print(name, "frames", len(frames), "min gap %.1f ms" % (min(gaps)*1000), "start", frames[0][1], "end", frames[-1][1])
