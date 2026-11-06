#!/usr/bin/env python3
"""Reads film mode and tone settings from Fuji RAFs without downloading them (M3-13).

A RAF starts with the camera's own JPEG, and that JPEG's EXIF holds the settings, so a small HTTP
range request is enough. Standard library only; nothing downloaded is ever executed.

    python3 -I pv-raf-survey.py urls.txt survey.jsonl     # one RAF URL per line
    python3 -I pv-raf-survey.py --local a.RAF b.RAF       # prints the same fields for local files
"""
import json, struct, sys, urllib.request
from concurrent.futures import ThreadPoolExecutor

HEAD = 90_000  # RAF header + EXIF segment (at most 64 KB) of the JPEG that follows it
AGENT = 'Pairview look fitting (personal project)'

FILM_MODE = {0x000: 'Provia', 0x100: 'Studio Portrait', 0x110: 'Studio Portrait EnhSat',
             0x120: 'Astia', 0x130: 'Studio Portrait IncSharp', 0x200: 'Velvia',
             0x300: 'Studio Portrait Ex', 0x400: 'Velvia', 0x500: 'Pro Neg Std',
             0x501: 'Pro Neg Hi', 0x600: 'Classic Chrome', 0x700: 'Eterna',
             0x800: 'Classic Neg', 0x900: 'Bleach Bypass', 0xA00: 'Nostalgic Neg',
             0xB00: 'Reala Ace'}
# Monochrome modes are in the Saturation tag; every other value there is ordinary saturation.
MONO = {0x300: 'BW', 0x301: 'BW Red', 0x302: 'BW Yellow', 0x303: 'BW Green', 0x310: 'BW Sepia',
        0x500: 'Acros', 0x501: 'Acros Red', 0x502: 'Acros Yellow', 0x503: 'Acros Green'}


def ifd(buf, at, order):
    """Entries of the IFD at `at` as {tag: (type, count, offset of its value)}."""
    n = struct.unpack_from(order + 'H', buf, at)[0]
    out = {}
    for i in range(n):
        p = at + 2 + 12 * i
        tag, kind, count = struct.unpack_from(order + 'HHI', buf, p)
        size = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 7: 1, 9: 4, 10: 8}.get(kind, 1) * count
        value_at = p + 8 if size <= 4 else struct.unpack_from(order + 'I', buf, p + 8)[0]
        out[tag] = (kind, count, value_at)
    return out


def value(buf, entry, order, base=0):
    kind, count, at = entry
    at += base
    if kind == 2:
        return buf[at:at + count].split(b'\0')[0].decode('latin-1').strip()
    if kind == 3:
        return struct.unpack_from(order + 'H', buf, at)[0]
    if kind == 4:
        return struct.unpack_from(order + 'I', buf, at)[0]
    if kind == 9:
        return struct.unpack_from(order + 'i', buf, at)[0]
    if kind == 5:
        a, b = struct.unpack_from(order + 'II', buf, at)
        return a / b if b else 0.0
    if kind == 10:
        a, b = struct.unpack_from(order + 'ii', buf, at)
        return a / b if b else 0.0
    return None


def parse(head):
    """Fields from the first bytes of a RAF, or raises if it isn't one we can read."""
    if not head.startswith(b'FUJIFILMCCD-RAW'):
        raise ValueError('not a RAF')
    jpeg_at, jpeg_len = struct.unpack_from('>II', head, 84)
    p = jpeg_at + 2
    while True:
        marker, size = struct.unpack_from('>HH', head, p)
        if marker == 0xFFE1 and head[p + 4:p + 10] == b'Exif\0\0':
            break
        if marker < 0xFF00 or marker == 0xFFDA:
            raise ValueError('no EXIF in the embedded JPEG')
        p += 2 + size
    tiff = head[p + 10:]
    order = '<' if tiff[:2] == b'II' else '>'
    top = ifd(tiff, struct.unpack_from(order + 'I', tiff, 4)[0], order)
    exif = ifd(tiff, value(tiff, top[0x8769], order), order)
    make = value(tiff, top[0x10F], order)
    model = value(tiff, top[0x110], order)
    out = {'make': make, 'model': model, 'jpeg_bytes': jpeg_len}
    # colorspace: 1 is sRGB; anything else (Adobe RGB) would skew a fit against sRGB-decoded values.
    for key, tag in (('iso', 0x8827), ('ev_bias', 0x9204), ('w', 0xA002), ('h', 0xA003),
                     ('colorspace', 0xA001)):
        if tag in exif:
            out[key] = value(tiff, exif[tag], order)
    if 0x9003 in exif:
        out['taken'] = value(tiff, exif[0x9003], order)
    maker_at = exif[0x927C][2]
    maker = tiff[maker_at:]
    if not maker.startswith(b'FUJIFILM'):
        raise ValueError('no Fuji makernote')
    tags = ifd(maker, struct.unpack_from('<I', maker, 8)[0], '<')
    get = lambda tag: value(maker, tags[tag], '<') if tag in tags else None
    sat = get(0x1003)
    mono = MONO.get(sat)
    fm = get(0x1401)
    out['film'] = mono or FILM_MODE.get(fm, f'unknown {fm}') if (mono or fm is not None) else None
    out.update(saturation=None if mono else sat, shadow=get(0x1040), highlight=get(0x1041),
               dr_auto=get(0x1402), dr_dev=get(0x1403), dr_auto_result=get(0x140B), chrome=get(0x1048), chrome_blue=get(0x104E),
               wb=get(0x1002), grain=get(0x1047))
    return out


def fetch(url):
    req = urllib.request.Request(url, headers={'Range': f'bytes=0-{HEAD - 1}', 'User-Agent': AGENT})
    with urllib.request.urlopen(req, timeout=60) as r:
        size = r.headers.get('Content-Range', '/').split('/')[-1]
        head = r.read(HEAD)
    return head, int(size) if size.isdigit() else None


def survey(url):
    try:
        head, size = fetch(url)
        return {'url': url, 'bytes': size, **parse(head)}
    except Exception as e:  # one unreadable sample doesn't stop a survey
        return {'url': url, 'error': f'{type(e).__name__}: {e}'}


def main():
    if sys.argv[1] == '--local':
        for path in sys.argv[2:]:
            with open(path, 'rb') as f:
                print(json.dumps({'path': path, **parse(f.read(HEAD))}))
        return
    urls = [u.strip() for u in open(sys.argv[1]) if u.strip()]
    with ThreadPoolExecutor(4) as pool, open(sys.argv[2], 'w') as out:
        for row in pool.map(survey, urls):
            out.write(json.dumps(row) + '\n')
            out.flush()


if __name__ == '__main__':
    main()
