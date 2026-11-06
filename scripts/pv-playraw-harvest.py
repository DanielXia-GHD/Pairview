#!/usr/bin/env python3
"""Finds RAF attachments and licence text in pixls.us Play Raw topics (M3-13).

Walks the category's listing pages and reads each topic's first post, slowly, using only paths the
forum's robots.txt allows (not /search). Standard library only.

    python3 -I pv-playraw-harvest.py <out dir> [max topics]

Writes <out dir>/playraw_posts.jsonl. Whether a post's licence allows a use is checked afterwards,
and by eye for every frame actually used.
"""
import html, json, os, re, sys, time, urllib.error, urllib.request

HEADERS = {'User-Agent': 'Pairview look fitting (personal project)', 'Accept': 'application/json'}
CATEGORY = 'https://discuss.pixls.us/c/processing/playraw/30.json?page={}'
TOPIC = 'https://discuss.pixls.us/t/{}.json'
PAUSE = 2


def get(url):
    for attempt in range(6):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS), timeout=30) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            if e.code != 429:
                return None
            time.sleep(30 * (attempt + 1))
        except OSError:
            time.sleep(5)
    return None


def main():
    out_dir, limit = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else None
    os.makedirs(out_dir, exist_ok=True)
    topics, page = {}, 0
    while limit is None or len(topics) < limit:
        listing = get(CATEGORY.format(page))
        found = listing['topic_list']['topics'] if listing else []
        if not found:
            break
        topics.update((t['id'], t['title']) for t in found)
        page += 1
        time.sleep(PAUSE)
    print(len(topics), 'topics', flush=True)
    with open(os.path.join(out_dir, 'playraw_posts.jsonl'), 'a') as out:
        for tid, title in list(topics.items())[:limit]:
            topic = get(TOPIC.format(tid))
            time.sleep(PAUSE)
            if not topic:
                continue
            post = topic['post_stream']['posts'][0]
            links = re.findall(r'href="([^"]+)"', post['cooked'])
            rafs = [h for h in links if re.search(r'\.raf(\?|$)', h, re.I)]
            if not rafs:
                continue
            text = html.unescape(re.sub(r'<[^>]+>', ' ', post['cooked']))
            licence = [text[max(0, m.start() - 50):m.end() + 60].strip()
                       for m in re.finditer(r'(?i)creative commons|licen[sc]ed?|\bCC[- ]?(BY|0)\b|public domain', text)]
            licence += [h for h in links if 'creativecommons.org' in h]
            out.write(json.dumps({'topic': tid, 'title': title, 'user': post['username'],
                                  'rafs': rafs, 'licence': licence}) + '\n')
            out.flush()
    print('done', flush=True)


if __name__ == '__main__':
    main()
