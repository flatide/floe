#!/usr/bin/env python3
"""Real CLI IPC -> off-reactor preparation -> owner HTTP -> native view gate."""
import hashlib
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile

from validate_web_cli import APP, INDEX, RENDERD, Client, read_json, wait


def digest(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


def main(fixture):
    # Short path also fits Darwin sockaddr_un; never claim the user's real key.
    with tempfile.TemporaryDirectory(prefix='fwh.', dir='/tmp') as td:
        work = Path(td).resolve()
        designs = work / 'designs'
        designs.mkdir()
        source = designs / '설계 with spaces.oas'
        shutil.copy2(fixture, source)
        temps = work / 'workers'
        temps.mkdir()
        env = dict(os.environ, PATH='', TMPDIR=str(temps),
                   FLOE_WEB_INSTANCE_DIR=str(work), DISPLAY=':floe-handoff-test',
                   FLOE_INDEX_BIN=str(INDEX), FLOE_RENDERD_BIN=str(RENDERD))
        for key in ('FLOE_FILL_EDIT', 'FLOE_JOBDECK_LEVELS', 'FLOE_FIREFOX_BIN'):
            env.pop(key, None)
        subprocess.run([str(APP), 'index', str(source), '--jobs', '2'], env=env,
                       check=True, capture_output=True, timeout=60)
        other = designs / 'other.oas'
        shutil.copy2(source, other)
        shutil.copytree(Path(str(source) + '.floe'), Path(str(other) + '.floe'))
        missing = designs / 'unindexed.oas'
        shutil.copy2(source, missing)
        deck = designs / 'two-levels.jb'
        deck.write_text('MTITLE 1,ONE\nMTITLE 2,TWO\nCHIP C\n'
                        '$ (1,P1,TC=other.oas,AD=0.001,LY={1},DT={0},UX=500,UY=500)\n'
                        '$ (2,P2,TC=other.oas,AD=0.001,LY={2},DT={0},UX=500,UY=500)\nROWS 0/0\n')
        # No new cache/write is permitted after the explicit setup index above.
        before = digest(designs)
        log = work / 'owner.stderr'
        with log.open('w') as stderr:
            proc = subprocess.Popen([str(APP), 'view', '--no-open'], env=env,
                                    stdout=subprocess.PIPE, stderr=stderr, text=True)
            try:
                def session():
                    m = re.search(r'private session link: (.+) \(one use', log.read_text())
                    return read_json(Path(m[1])) if m else None
                credentials = wait(session, proc)
                client = Client(credentials)
                client.call('GET', '/api/v1/launch', code=401)
                client.login()
                assert client.call('GET', '/api/v1/capabilities')['launcher']
                assert client.call('GET', '/api/v1/catalog')['sources'] == []
                assert client.call('GET', '/api/v1/startup')['request'] is None
                assert client.call('GET', '/api/v1/operations')['last_seq'] == '0'

                def forward(*args, success=True, policy=None):
                    # Existing owner discovery must precede native/browser discovery.
                    sender = dict(env, FLOE_RENDERD_BIN='/invalid/renderd',
                                  FLOE_INDEX_BIN='/invalid/index', FLOE_FIREFOX_BIN='/invalid/firefox')
                    if policy is not None:
                        sender['FLOE_JOBDECK_LEVELS'] = policy
                    result = subprocess.run([str(APP), *args], env=sender,
                                            capture_output=True, text=True, timeout=8)
                    assert (result.returncode == 0) is success, (result.returncode, result.stderr)
                    assert credentials['url'] not in result.stdout + result.stderr
                    if success:
                        assert 'queued' in result.stdout

                def proposal(phase='ready'):
                    return wait(lambda: (lambda p: p if p and p['phase'] == phase else None)(
                        client.call('GET', '/api/v1/launch')['pending']), proc)

                def open_proposal(p, seq, current=None, succeeds=True):
                    body = dict(action='open', seq=str(seq), pixels=[320, 240], levels=p['request']['levels'])
                    if current:
                        body.update(view_id=current['view_id'], state_rev=current['state_rev'])
                    path = '/api/v1/launch/' + p['id']
                    receipt = client.call('POST', path, body)
                    assert receipt['phase'] == 'submitted'
                    assert client.call('GET', path)['receipt'] == receipt
                    assert client.call('POST', path, body) == receipt
                    status = client.finished(seq, proc)
                    assert status['phase'] == ('succeeded' if succeeds else 'failed'), status
                    if not succeeds:
                        return status
                    return wait(lambda: (lambda v: v if v['status'] == 'idle' else None)(
                        client.call('GET', '/api/v1/view')['view']), proc)

                forward('view', str(source), '--goto', '1.25,-2.5,700', '--thin', 'keep', '--detail', 'high')
                p = proposal()
                assert str(designs) not in str(p), 'filesystem path crossed owner boundary'
                assert p['request']['body']['depth'] == 'full'
                assert p['request']['body']['font_px'] == 14
                forward('view', str(other), success=False)  # pending request is bounded
                first = open_proposal(p, 1)
                assert first['effective_thin'] == 'keep' and first['detail'] == 'high'
                assert first['pixels'] == [320, 240]
                assert int(first['submitted']) - int(first['margin_submitted']) == 1

                forward(str(source), '--goto', '5,6,300', '--thin', 'auto')
                p = proposal()
                assert p['request']['body']['detail'] == 'medium'
                assert p['request']['body']['thin'] == 'auto'
                second = open_proposal(p, 2, first)
                assert (second['view_id'], second['worker_epoch']) == (first['view_id'], first['worker_epoch'])
                assert second['effective_thin'] == 'cull'
                assert second['detail'] == 'medium'
                # Effective refinement on is also omission for instance
                # ownership. Invalid sender tools prove no replacement worker
                # or browser is discovered before the existing owner is used.
                for present in ((), ('view', '--refinement', 'on'),
                                ('view', '--refinement', 'off', '--refinement', 'on')):
                    forward(*present)
                    p = proposal()
                    assert p['request'] is None
                    client.call('POST', '/api/v1/launch/' + p['id'], dict(action='present'))
                    assert client.call('GET', '/api/v1/operations')['last_seq'] == '2'
                    assert client.call('GET', '/api/v1/view')['view']['render_rev'] == second['render_rev']

                forward('view', str(missing))
                failed = open_proposal(proposal(), 3, second, succeeds=False)
                assert failed['error'] == 'index_unavailable'
                assert client.call('GET', '/api/v1/view')['view']['view_id'] == second['view_id']
                forward('view', str(other), '--goto', '0,0,600')
                third = open_proposal(proposal(), 4, second)
                assert third['view_id'] != second['view_id']

                forward('view', str(designs / 'not-found.oas'))
                p = proposal('failed')
                assert p['request'] is None and str(designs) not in str(p)
                client.call('POST', '/api/v1/launch/' + p['id'], dict(action='dismiss'))

                for extra, policy, ask, selected in [([], None, True, dict(mode='all')),
                        ([], '2', False, dict(mode='only', ids=['2'])),
                        (['--level', '1'], 'invalid', False, dict(mode='only', ids=['1']))]:
                    forward(str(deck), *extra, policy=policy)
                    p = proposal()
                    assert p['confirm_levels'] is ask
                    assert p['request']['levels'] == selected
                    assert p['request']['body']['labels'] is False
                    assert p['request']['label_preference'] is True
                    client.call('POST', '/api/v1/launch/' + p['id'], dict(action='dismiss'))

                isolated_file = work / 'independent.json'
                independent = subprocess.Popen([str(APP), 'view', '--multi', '--no-open',
                    '--session-file', str(isolated_file)], env=env, stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE, text=True)
                try:
                    isolated = Client(wait(lambda: read_json(isolated_file), independent))
                    isolated.login()
                    assert not isolated.call('GET', '/api/v1/capabilities')['launcher']
                    assert isolated.origin != client.origin
                    isolated.call('DELETE', '/api/v1/session', code=204)
                    independent.communicate(timeout=15)
                    assert independent.returncode == 0
                finally:
                    if independent.poll() is None:
                        independent.terminate()
                        independent.communicate(timeout=15)
                assert client.call('GET', '/api/v1/operations')['last_seq'] == '4'
                client.call('DELETE', '/api/v1/session', code=204)
                proc.communicate(timeout=15)
                assert proc.returncode == 0, log.read_text()
                assert not list(temps.iterdir()), 'worker/session files leaked'
                assert not [p for p in work.rglob('*') if p.is_socket()], 'IPC socket leaked'
                assert digest(designs) == before, 'handoff modified a source/cache or silently indexed'
            finally:
                if proc.poll() is None:
                    proc.send_signal(signal.SIGINT)
                    proc.communicate(timeout=15)
        print('WEB HANDOFF: ALL OK (empty owner, queue/busy, first frame, cache-preserving goto, present, failure preservation, replacement, independent, receipt, cleanup)')


if __name__ == '__main__':
    main(Path(sys.argv[1]).resolve())
