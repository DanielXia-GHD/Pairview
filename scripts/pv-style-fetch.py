# Downloads manifest frames, each source in its own folder. Image files only; nothing is executed.
import os,sys,time,urllib.request
D=sys.argv[1]; UA={'User-Agent':'Pairview look fitting (personal project)'}
for l in open(D+'/manifest.tsv'):
    look,src,fid,_,url,*_=l.rstrip('\n').split('\t')
    d=f'{D}/frames/{src}'; os.makedirs(d,exist_ok=True)
    p=f'{d}/{fid}.RAF'
    if os.path.exists(p): continue
    try:
        with urllib.request.urlopen(urllib.request.Request(url,headers=UA),timeout=120) as r, open(p+'.part','wb') as f:
            while True:
                b=r.read(1<<20)
                if not b: break
                f.write(b)
        with open(p+'.part','rb') as f:
            assert f.read(15)==b'FUJIFILMCCD-RAW','not a RAF'
        os.replace(p+'.part',p); print('ok',fid,flush=True)
    except Exception as e:
        print('FAILED',fid,e,flush=True)
        if os.path.exists(p+'.part'): os.remove(p+'.part')
    time.sleep(1)
print('fetch finished',flush=True)
