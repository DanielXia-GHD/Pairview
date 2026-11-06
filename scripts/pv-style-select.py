# Chooses one frame per scene for each look and writes manifest.tsv. Tier: score = plain settings
# (any DR, RapidRAW compensates it; any colour space, converted), train = mild tone settings.
import json,sys,random,collections,re
D=sys.argv[1]; S=D+'/survey/'
rows=json.load(open(S+'all.json'))
meta=json.load(open(S+'playraw_meta.json'))
try: meta.update(json.load(open(S+'part2/playraw_meta.json')))
except FileNotFoundError: pass
caps={'Provia':40,'Velvia':24,'Classic Chrome':24,'Astia':24}
EXCLUDE_TOPICS={9571}          # infrared-converted camera
EXCLUDE_MODELS={'X-S1'}
mild=lambda v: v in (None,0,16,-16)
def plain(r): return not r.get('saturation') and r.get('shadow') in (None,0) and r.get('highlight') in (None,0) and not r.get('chrome') and not r.get('chrome_blue')
def soft(r): return r.get('saturation') in (None,0,128,384) and mild(r.get('shadow')) and mild(r.get('highlight')) and r.get('chrome') in (None,0,32) and r.get('chrome_blue') in (None,0,32)
random.seed(13)
out=[]
for look,cap in caps.items():
    c=[r for r in rows if r['film']==look and r['model'] not in EXCLUDE_MODELS and meta.get(r['url'],{}).get('topic') not in EXCLUDE_TOPICS and (plain(r) or soft(r))]
    scenes=collections.defaultdict(list)
    for r in c: scenes[(r['model'],(r.get('taken') or r['url'])[:13])].append(r)
    keys=sorted(scenes); random.shuffle(keys); keys.sort(key=lambda k:0 if any(plain(r) for r in scenes[k]) else 1); keys=keys[:cap]
    for k in keys:
        r=sorted(scenes[k],key=lambda r:(not plain(r),r['url']))[0]
        m=meta.get(r['url'],{})
        rid=re.sub(r'[^A-Za-z0-9]+','_',r['url'].rsplit('/',1)[1].rsplit('.',1)[0])[-28:]
        lic='CC0' if r['src']=='pixls-cc0' else m.get('licence','?')
        out.append([look,r['src'],f"{r['model'].replace(' ','')}_{rid}",'score' if plain(r) else 'train',r['url'],lic,str(m.get('topic','')),r['model'],str(r.get('bytes')),str(r.get('colorspace',1))])
    print(f"{look:16} scenes {len(scenes):3} used {len(keys):3}, plain {sum(1 for x in out if x[0]==look and x[3]=='score')}")
open(D+'/manifest.tsv','w').write('\n'.join('\t'.join(x) for x in out)+'\n')
print(len(out),'frames',sum(int(x[8]) for x in out)/1e9,'GB',collections.Counter(x[5] for x in out))
