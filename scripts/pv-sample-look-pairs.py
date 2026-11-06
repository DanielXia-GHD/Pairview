#!/usr/bin/env python3
"""Picks ARW+HIF pairs across shoots and copies them locally for fitting looks (M3-06).

Read-only on the NAS, and resumable. Only with the owner's go-ahead, since it reads the NAS:

    python3 pv-sample-look-pairs.py /Volumes/Public/Photos/A/DCIM <local dir>/pairs 60

Then put look-neutral.json next to `pairs` as neutral.json and see src-tauri/src/pv/look_job.rs.
"""
import os, sys, shutil, random, time, json
root, dest, want = sys.argv[1], sys.argv[2], int(sys.argv[3])
assert root.startswith('/Volumes/') and not dest.startswith('/Volumes/')
random.seed(7)
shoots = []
for name in sorted(os.listdir(root)):
    s = os.path.join(root, name)
    arw, hif = os.path.join(s, 'arw'), os.path.join(s, 'hif')
    if not (os.path.isdir(arw) and os.path.isdir(hif)): continue
    a = {os.path.splitext(f)[0]: f for f in os.listdir(arw) if f.lower().endswith('.arw')}
    h = {os.path.splitext(f)[0]: f for f in os.listdir(hif) if f.lower().endswith(('.hif','.heif'))}
    stems = sorted(set(a) & set(h))
    if stems: shoots.append((name, arw, hif, a, h, stems))
print(len(shoots), 'ARW+HIF shoots', flush=True)
picks = []
for name, arw, hif, a, h, stems in shoots:
    random.shuffle(stems)
    picks.append([(name, os.path.join(arw, a[s]), os.path.join(hif, h[s]), s) for s in stems])
chosen, i = [], 0
while len(chosen) < want and any(picks):
    p = picks[i % len(picks)]
    if p: chosen.append(p.pop())
    i += 1
t0 = time.time(); total = 0
for n, (name, a, h, s) in enumerate(chosen):
    d = os.path.join(dest, f'{name}__{s}'); os.makedirs(d, exist_ok=True)
    for src, out in ((a, 'raw.ARW'), (h, 'cam.HIF')):
        o = os.path.join(d, out)
        if os.path.exists(o) and os.path.getsize(o) == os.path.getsize(src): continue
        shutil.copyfile(src, o + '.part'); os.replace(o + '.part', o)
        total += os.path.getsize(o)
    print(f'{n+1}/{len(chosen)} {name} {s} {total/1e6:.0f} MB {time.time()-t0:.0f}s', flush=True)
print('done', flush=True)
