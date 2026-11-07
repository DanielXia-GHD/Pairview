#!/usr/bin/env python3
"""Reads what a DNG holds without downloading it, and takes out its camera JPEG (M3-14).

Modern Leica DNGs embed a near-full-size JPEG rendered by the camera, so one file is a whole pair.
Standard library only; nothing downloaded is ever executed.

    python3 -I pv-dng-survey.py survey urls.txt survey.jsonl    # one DNG URL per line
    python3 -I pv-dng-survey.py extract in.DNG out.jpg          # writes the largest preview

A DNG re-saved by Adobe software has Adobe's rendering as its preview, not the camera's: `software`
and `preview_app` are there to catch that. Greyscale is measured on the smallest JPEG preview, which
is enough to tell a B&W film mode from a colour one.
"""
import json, struct, sys, urllib.request
from concurrent.futures import ThreadPoolExecutor

AGENT = 'Pairview look fitting (personal project)'
HEAD = 600_000
SIZES = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}
JPEG_COMPRESSION = 7
PREVIEW_SUBFILE = 1  # NewSubfileType: a reduced-size copy, not the raw data


def entries(buf, at, order):
    """The IFD at `at` as {tag: (type, count, offset of its value)}."""
    count = struct.unpack_from(order + 'H', buf, at)[0]
    out = {}
    for i in range(count):
        p = at + 2 + 12 * i
        tag, kind, n = struct.unpack_from(order + 'HHI', buf, p)
        size = SIZES.get(kind, 1) * n
        out[tag] = (kind, n, p + 8 if size <= 4 else struct.unpack_from(order + 'I', buf, p + 8)[0])
    return out


def read(buf, entry, order):
    """A tag's value: text, one number, or a list of numbers. None if it's past the buffer."""
    if entry is None:
        return None
    kind, n, at = entry
    if at + SIZES.get(kind, 1) * n > len(buf):
        return None
    if kind == 2:
        return buf[at:at + n].split(b'\0')[0].decode('latin-1').strip()
    if kind in (1, 7) and n <= 64:
        return buf[at:at + n].hex()
    code = {3: 'H', 4: 'I', 8: 'h', 9: 'i'}.get(kind)
    if code:
        values = struct.unpack_from(order + code * n, buf, at)
        return values[0] if n == 1 else list(values)
    return None


def previews(buf, order):
    """(top IFD, every baseline-JPEG reduced copy in IFD0 and its SubIFDs, smallest first)."""
    top = entries(buf, struct.unpack_from(order + 'I', buf, 4)[0], order)
    found, places = [], [('ifd0', top)]
    sub = read(buf, top.get(330), order)
    for i, offset in enumerate(sub if isinstance(sub, list) else [sub] if sub else []):
        if offset < len(buf):
            places.append((f'sub{i}', entries(buf, offset, order)))
    for name, ifd in places:
        if read(buf, ifd.get(259), order) != JPEG_COMPRESSION:
            continue
        # IFD0's thumbnail has no NewSubfileType on some bodies; the raw data always says 0.
        kind = read(buf, ifd.get(254), order)
        if name != 'ifd0' and kind != PREVIEW_SUBFILE:
            continue
        if name == 'ifd0' and kind == 0:
            continue
        at, length = read(buf, ifd.get(273), order), read(buf, ifd.get(279), order)
        if at is None or length is None:
            continue
        at = at[0] if isinstance(at, list) else at
        length = length[0] if isinstance(length, list) else length
        found.append({'in': name, 'w': read(buf, ifd.get(256), order),
                      'h': read(buf, ifd.get(257), order), 'at': at, 'len': length,
                      'colorspace': read(buf, ifd.get(0xC71A), order),
                      'preview_app': read(buf, ifd.get(0xC716), order)})
    found.sort(key=lambda p: p['w'] or 0)
    return top, found


def parse(buf):
    if buf[:2] not in (b'II', b'MM'):
        raise ValueError('not a TIFF')
    order = '<' if buf[:2] == b'II' else '>'
    top, found = previews(buf, order)
    out = {'make': read(buf, top.get(271), order), 'model': read(buf, top.get(272), order),
           'software': read(buf, top.get(305), order), 'orientation': read(buf, top.get(274), order),
           'preview_app': read(buf, top.get(0xC716), order),
           'preview_colorspace': read(buf, top.get(0xC71A), order), 'previews': found}
    exif_at = read(buf, top.get(0x8769), order)
    if exif_at and exif_at < len(buf):
        exif = entries(buf, exif_at, order)
        out['taken'] = read(buf, exif.get(0x9003), order)
        out['iso'] = read(buf, exif.get(0x8827), order)
        note = exif.get(0x927C)
        if note and buf[note[2]:note[2] + 5] == b'LEICA':
            out['film_mode'] = leica_film_mode(buf, note[2], order)
    return out


def leica_film_mode(buf, at, order):
    """The film mode text. Which tag holds it depends on the body (0x412, or 0x34C on M bodies)."""
    try:
        tags = entries(buf, at + 8, order)
        for tag in (0x412, 0x34C):
            if tag in tags:
                return read(buf, tags[tag], order) or ''
    except struct.error:
        pass
    return None


def fetch(url, start, length):
    req = urllib.request.Request(url, headers={'User-Agent': AGENT,
                                               'Range': f'bytes={start}-{start + length - 1}'})
    with urllib.request.urlopen(req, timeout=60) as r:
        size = r.headers.get('Content-Range', '/').split('/')[-1]
        return r.read(length), int(size) if size.isdigit() else None


def survey(url):
    try:
        head, size = fetch(url, 0, HEAD)
        return {'url': url, 'bytes': size, **parse(head)}
    except Exception as e:  # one unreadable sample doesn't stop a survey
        return {'url': url, 'error': f'{type(e).__name__}: {e}'}


def extract(source, target):
    with open(source, 'rb') as f:
        data = f.read()
    _, found = previews(data, '<' if data[:2] == b'II' else '>')
    if not found:
        raise SystemExit(f'{source}: no JPEG preview')
    best = found[-1]
    jpeg = data[best['at']:best['at'] + best['len']]
    if jpeg[:2] != b'\xff\xd8':
        raise SystemExit(f'{source}: the preview is not a JPEG')
    with open(target, 'wb') as f:
        f.write(jpeg)


def main():
    if sys.argv[1] == 'extract':
        extract(sys.argv[2], sys.argv[3])
        return
    urls = [u.strip() for u in open(sys.argv[2]) if u.strip()]
    with ThreadPoolExecutor(2) as pool, open(sys.argv[3], 'w') as out:
        for row in pool.map(survey, urls):
            out.write(json.dumps(row) + '\n')
            out.flush()


if __name__ == '__main__':
    main()
