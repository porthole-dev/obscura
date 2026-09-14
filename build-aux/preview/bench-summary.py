#!/usr/bin/env python3
"""bench-summary.py LOG...: tables from obscura-perf lines."""
import re, statistics, sys

def events(path):
    out = []
    for line in open(path, errors="replace"):
        m = re.search(r"obscura-perf ([\d.]+) (\S+) ?(.*)", line)
        if m:
            out.append((float(m[1]), m[2], m[3]))
    return out

def first(ev, name):
    return next((t for t, n, _ in ev if n == name), None)

def detail(ev, name, key):
    for _, n, d in ev:
        if n == name and (m := re.search(key + r"=([\d.-]+)", d)):
            return float(m[1])

def med(xs):
    xs = [x for x in xs if x is not None]
    return f"{statistics.median(xs):.0f}" if xs else "-"

rows, photos, switches, vf, rec = [], [], [], [], []
for path in sys.argv[1:]:
    ev = events(path)
    exec_ms = detail(ev, "main", "exec_to_main_ms") or 0
    rows.append([path.rsplit("/", 1)[-1], exec_ms] + [first(ev, n) for n in
                ("window-mapped", "window-painted", "portal", "camera-manager", "camera-started", "frame-first-queued", "frame-first-presented")])
    shutters = [t for t, n, _ in ev if n == "shutter"]
    for s in shutters:
        nxt = lambda name: next((t - s for t, n, _ in ev if n == name and t > s), None)
        photos.append((nxt("still-received"), nxt("photo-jpeg-written"), nxt("thumbnail-shown"), nxt("thumbnail-preview"), nxt("viewfinder-restored"), nxt("still-reconfigure")))
    reqs = [t for t, n, d in ev if n == "camera-open-request"]
    for r in reqs[1:]:
        switches.append(next((t - r for t, n, _ in ev if n == "frame-first-presented" and t > r), None))
    for _, n, d in ev:
        if n == "viewfinder":
            vf.append({k: float(v) for k, v in re.findall(r"(\w+)=([\d.]+)", d)})
    for a, b in (("record-press", "recording-started"), ("record-stop-press", "recording-saved")):
        ta, tb = first(ev, a), first(ev, b)
        if ta is not None and tb is not None:
            rec.append((a, tb - ta))

cols = ["run", "exec>main", "mapped", "painted", "portal", "cam-mgr", "cam-started", "1st-queued", "1st-shown"]
print("startup, ms since main (exec>main is before main)")
print(" | ".join(f"{c:>11}" for c in cols))
for r in rows:
    print(" | ".join(f"{r[0]:>11}" if i == 0 else f"{('-' if v is None else f'{v:.0f}'):>11}" for i, v in enumerate(r)))
if photos:
    print(f"\nphotos x{len(photos)}, median ms after shutter: preview thumbnail {med([p[3] for p in photos])}, still {med([p[0] for p in photos])}, jpeg written {med([p[1] for p in photos])}, saved thumbnail {med([p[2] for p in photos])}")
    full = [p for p in photos if p[5] is not None and (p[0] is None or p[5] < p[0])]
    if full:
        print(f"  full-resolution x{len(full)}: still {med([p[0] for p in full])}, viewfinder back {med([p[4] for p in full])}; fast shutter x{len(photos) - len(full)}")
if switches:
    print(f"camera/mode switches x{len(switches)}, median ms to first frame shown: {med(switches)} (all: {', '.join(med([s]) for s in switches)})")
if vf:
    key = lambda k: med([v.get(k) for v in vf])
    print(f"viewfinder ({len(vf)} windows of 2 s), medians: delivered {key('delivered_fps')} fps, presented {key('presented_fps')} fps, "
          f"dropped {key('dropped')}/window, main thread {statistics.median([v['main_ms_avg'] for v in vf]):.2f} ms avg / {statistics.median([v['main_ms_max'] for v in vf]):.2f} ms max per frame, "
          f"RSS max {max(v.get('rss_mib', 0) for v in vf):.0f} MiB")
for name, ms in rec:
    print(f"{name} -> done: {ms:.0f} ms")
