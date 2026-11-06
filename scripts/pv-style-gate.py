# Marks frames outside normal brightness as excluded: near-black colours can hardly differ, so they
# flatter any score, and blown-out frames have nothing to fit.
import sys,numpy as np
from PIL import Image
D=sys.argv[1]; LO,HI=0.12,0.85
rows=[l.rstrip('\n').split('\t') for l in open(D+'/manifest.tsv')]
n=0
for r in rows:
    try: c=np.asarray(Image.open(f'{D}/work/{r[2]}/camera.png')).astype(float)/255
    except FileNotFoundError: continue
    m=float(np.median((c*[.2126,.7152,.0722]).sum(-1)))
    if not LO<=m<=HI:
        print('exclude',r[0],r[2],round(m,3)); r[3]='exclude'; n+=1
open(D+'/manifest.tsv','w').write('\n'.join('\t'.join(x) for x in rows)+'\n')
print(n,'excluded of',len(rows))
