#!/usr/bin/env python3
"""Authenticated explicit pack build/adoption: native app, synthetic files only."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from validate_web_cli import APP, INDEX, RENDERD, ROOT, Client, read_json, wait
from validate_drc_build import fingerprint, clean_stage

DB = 'TOP 1000\nWIDTH\n2 2 1\nwidth rule\np 1 4\n0 0\n100 0\n100 20\n0 20\ne 2 1\n50 0 50 100\n'


def drc_ready(c, p):
    return wait(lambda: (lambda r: r if r and r['phase'] == 'ready' else None)(
        c.call('GET', '/api/v1/drc')['drc']), p)


def done(c, p, seq):
    return wait(lambda: (lambda r: r if r['phase'] in ('succeeded', 'failed', 'cancelled') else None)(
        c.call('GET', '/api/v1/drc/builds/' + str(seq))), p, 30)


def request(drc, view, seq, force=False, jobs=2):
    return dict(seq=str(seq), drc_id=drc['id'], revision=drc['revision'],
                view_id=view, approve=True, force=force, jobs=jobs)


def main(fixture):
    with tempfile.TemporaryDirectory(prefix='floe-web-drc-build-') as td:
        root = Path(td).resolve()
        source = root / 'layout.oas'
        shutil.copy2(fixture, source)
        subprocess.run([str(INDEX), 'vfs', str(source), str(source)+'.floe', '--jobs', '2'],
                       capture_output=True, check=True, timeout=30)
        version = subprocess.check_output([str(INDEX), '--version'], text=True).split()[1]
        fake = root / 'controlled native'
        fake.write_text(f'''#!{sys.executable}
import os,pathlib,signal,sys,time
if sys.argv[1:]==['--version']:
    print('floe-index {version}');sys.exit(0)
assert sys.argv[1]=='drc',sys.argv
src,dst=map(pathlib.Path,sys.argv[2:4])
mode=pathlib.Path(os.environ['DRC_BUILD_MODE']).read_text()
pathlib.Path(str(src)+'.pid').write_text(str(os.getpid()))
dst.write_bytes(b'FLOEICE\\0broken')
pathlib.Path(str(dst)+'.tmp-native').write_bytes(b'temporary')
print('[drc-pack enc] 1/2 checks, 123 errors, 0.0G blob',file=sys.stderr,flush=True)
if mode=='fail':sys.exit(7)
if mode=='corrupt':sys.exit(0)
signal.signal(signal.SIGTERM,signal.SIG_IGN)
while True:time.sleep(.01)
''')
        fake.chmod(0o700)
        for mode in ('native', 'faults', 'logout', 'ice', 'collision'):
            work = root / mode
            work.mkdir()
            db = work / '공백 results.db'
            db.write_text(DB)
            pack = Path(str(db)+'.ice')
            golden = work / 'golden.ice'
            subprocess.run([str(INDEX),'drc',str(db),str(golden)], capture_output=True,check=True,timeout=20)
            if mode != 'native': shutil.copy2(golden,pack)
            if mode == 'collision':
                pack.write_text('{"format":"floe-svrf-rules","version":1,"checks":{}}')
            protected = work / 'review.notes'
            protected.write_bytes(b'untouched review')
            original = fingerprint(db)
            temps = work / 'temps'; temps.mkdir()
            session = work / 'session.json'
            modefile = work / 'mode'; modefile.write_text('fail')
            env = dict(os.environ, PATH='', FLOE_INDEX_BIN=str(fake if mode in ('faults','logout') else INDEX),
                       FLOE_RENDERD_BIN=str(RENDERD),TMPDIR=str(temps),DRC_BUILD_MODE=str(modefile))
            args = [str(APP),'view',str(source),'--no-open','--session-file',str(session),
                    '--drc',str(pack if mode=='ice' else db),'--jobs','2','--raster-jobs','1',
                    '--frame-cache','off','--no-labels']
            if mode == 'collision':
                alias=pack.with_suffix('.ICE')
                # On case-insensitive filesystems this is the same inode with
                # nlink=1, not caught by the native hard-link guard.
                args += ['--drc-rules',str(alias if alias.exists() else pack)]
            p = subprocess.Popen(args,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                c = Client(wait(lambda:read_json(session),p))
                asset = '/assets/' + c.bundle + '/drc-build.js'
                assert asset.encode() in c.call('GET','/')
                assert c.call('GET',asset) == (ROOT/'rust/web/ui/drc-build.js').read_bytes(), 'stale or missing embedded build controller'
                for method,path,body in [('GET','/api/v1/drc/builds',None),('POST','/api/v1/drc/builds',{}),
                                          ('GET','/api/v1/drc/builds/1',None),('POST','/api/v1/drc/builds/1/cancel',{})]:
                    c.call(method,path,body,401)
                c.login()
                drc = drc_ready(c,p)
                assert c.call('GET','/api/v1/drc')['build']['available'] is (mode!='ice')
                startup = c.call('GET','/api/v1/startup')['request']
                startup['body']['pixels'] = [257,191]
                c.call('POST','/api/v1/operations',startup,202)
                opened = c.finished(1,p); assert opened['phase']=='succeeded'
                view = opened['view_id']
                before = wait(lambda:(lambda v:v if v['status']=='idle' else None)(c.call('GET','/api/v1/view')['view']),p)
                def submit(req,code=202): return c.call('POST','/api/v1/drc/builds',req,code)
                if mode=='ice':
                    submit(request(drc,view,1),403)
                    assert c.call('GET','/api/v1/drc/builds')['last_seq']=='0'
                else:
                    first = request(drc,view,1)
                    csrf = c.csrf; c.csrf='bad'; submit(first,401); c.csrf=csrf
                    for edit,code in [({'approve':False},400),({'jobs':0},400),({'jobs':17},400),
                                      ({'path':'/etc/passwd'},400),({'revision':'old'},409),({'view_id':'old'},409)]:
                        submit(dict(first,**edit),code)
                    assert c.call('GET','/api/v1/drc/builds')['last_seq']=='0'
                    if mode=='collision':
                        old=fingerprint(pack)
                        submit(request(drc,view,1,True))
                        result=done(c,p,1)
                        assert result['phase']=='failed',result
                        assert fingerprint(pack)==old,'force overwrote the registered SVRF file'
                        assert drc_ready(c,p)['metadata']['format']=='ascii'
                    elif mode=='native':
                        for seq,force,jobs in [(1,False,2),(2,False,2),(3,True,4),(4,True,16)]:
                            old = drc; req=request(drc,view,seq,force,jobs)
                            old_pack = fingerprint(pack) if pack.exists() else None
                            selection=f"/api/v1/drc/{old['id']}/views/{view}/selection"
                            c.call('POST',selection,dict(revision=old['revision'],base_selection_rev='1',
                                   body=dict(kind='apply',check='0',errors=['0'],mode='add')))
                            submit(req)
                            result=done(c,p,seq)
                            assert result['phase']==('failed' if jobs==16 else 'succeeded'),result
                            if jobs==16: assert result['error']=='busy',result
                            if seq==2: assert result['outcome']['reused'] and fingerprint(pack)==old_pack
                            assert pack.read_bytes()==golden.read_bytes()
                            drc=drc_ready(c,p)
                            assert drc['id']!=old['id'] and drc['revision']!=old['revision']
                            assert drc['metadata']['format']=='ice'
                            assert c.call('GET',f"/api/v1/drc/{drc['id']}/views/{view}/selection")['state']['total']=='0'
                            assert c.call('GET',f"/api/v1/drc/{drc['id']}/views/{view}/panel")['state']['body'] is None
                            c.call('GET',selection,code=404)
                            c.call('POST',f"/api/v1/drc/{old['id']}/read",dict(view_id=view,revision=old['revision'],body=dict(kind='rule',check='0')),404)
                            assert submit(req)==result,'uncertain retry repeated write or returned different operation'
                            submit(dict(req,force=not req['force']),409)
                            assert c.call('GET','/api/v1/drc')['drc']['id']==drc['id']
                        db.write_text(DB.replace('100 20','100.25 20.5'))
                        old=fingerprint(pack); submit(request(drc,view,5,True))
                        result=done(c,p,5)
                        assert result['phase']=='failed' and result['noninteger'],result
                        assert fingerprint(pack)==old
                        drc=drc_ready(c,p)
                        assert drc['metadata']['format']=='ice'
                    else:
                        for seq,fault in enumerate(['hardhang'] if mode=='logout' else ['fail','corrupt','hardhang'],1):
                            marker=Path(str(db)+'.pid'); marker.unlink(missing_ok=True)
                            modefile.write_text(fault)
                            old=fingerprint(pack); req=request(drc,view,seq,True)
                            submit(req)
                            child=int(wait(lambda:marker.read_text() if marker.exists() else None,p))
                            if fault=='hardhang':
                                assert submit(req)['seq']==str(seq)
                                progress=wait(lambda:(lambda v:v if v.get('native',{}).get('checks')=='1' else None)(c.call('GET','/api/v1/drc/builds/'+str(seq))),p)
                                assert progress['native']['errors']=='123'
                                assert 'pid' not in str(progress) and str(db) not in str(progress)
                                if mode=='logout': c.call('DELETE','/api/v1/session',code=204)
                                else: c.call('POST',f'/api/v1/drc/builds/{seq}/cancel',{},202)
                            if mode!='logout':
                                result=done(c,p,seq)
                                assert result['phase']==('cancelled' if fault=='hardhang' else 'failed'),result
                                drc=drc_ready(c,p)
                                assert c.call('POST',f'/api/v1/drc/builds/{seq}/cancel',{},202)==result
                            else: p.communicate(timeout=10);assert p.returncode==0
                            assert fingerprint(pack)==old
                            try:os.kill(child,0)
                            except ProcessLookupError:pass
                            else:raise AssertionError('native child survived terminal state')
                            clean_stage(work)
                    if mode!='logout':
                        after=c.call('GET','/api/v1/view')['view']
                        for key in ('view_id','state_rev','render_rev','submitted','bbox_dbu','layers'):
                            assert after[key]==before[key],(key,after,before)
                if mode!='logout':c.call('DELETE','/api/v1/session',code=204)
                out,err=p.communicate(timeout=15);assert p.returncode==0,(out,err)
                assert c.token not in out+err
                assert not session.exists() and not list(temps.iterdir())
                assert protected.read_bytes()==b'untouched review'
                if mode!='native':assert fingerprint(db)==original
                clean_stage(work)
            finally:
                if p.poll() is None:
                    p.terminate()
                    try:p.communicate(timeout=15)
                    except subprocess.TimeoutExpired:p.kill();p.communicate(timeout=5)
    print('WEB DRC BUILD: ALL OK (approval/auth, native/reuse/force, budget, identities/groups, idempotency, faults/cancel/logout reap, layout unchanged)')


if __name__ == '__main__':
    main(Path(sys.argv[1]).resolve())
