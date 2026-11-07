# Downloads manifest frames, each source in its own folder. Image files only; nothing is executed.
import os,sys,time,urllib.request
D=sys.argv[1]; UA={'User-Agent':'Pairview look fitting (personal project)'}
for l in open(D+'/manifest.tsv'):
    look,src,fid,_,url,*_=l.rstrip('\n').split('\t')
    d=f'{D}/frames/{src}'; os.makedirs(d,exist_ok=True)
    p=f'{d}/{fid}.{url.rsplit(".",1)[1].upper()}'
    if os.path.exists(p): continue
    try:
        with urllib.request.urlopen(urllib.request.Request(url,headers=UA),timeout=120) as r, open(p+'.part','wb') as f:
            while True:
                b=r.read(1<<20)
                if not b: break
                f.write(b)
        with open(p+'.part','rb') as f:
            head=f.read(15)
            assert head==b'FUJIFILMCCD-RAW' or head[:2] in (b'II',b'MM'),'not a RAF or DNG'
        os.replace(p+'.part',p); print('ok',fid,flush=True)
    except Exception as e:
        print('FAILED',fid,e,flush=True)
        if os.path.exists(p+'.part'): os.remove(p+'.part')
    time.sleep(1)
print('fetch finished',flush=True)
