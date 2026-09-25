'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { ReviewIntercept } = require('./electron-review-intercept.cjs');
const origin = 'http://127.0.0.1:12345';
function request(kind = 'notes', extra = {}) {
  return { id: 1, webContentsId: 7, method: 'POST', url: origin + '/api/v1/drc/review/' + kind, ...extra };
}
function setup() { return new ReviewIntercept(() => origin, () => 7); }
test('only exact owned save endpoints count; read/prepare/other origins do not', () => {
  const w = setup();
  for (const d of [request('notes/prepare'), request('waives/read'), request('notes', {method:'GET'}),
    request('notes', {webContentsId:8}), request('notes', {url:'http://127.0.0.1:12346/api/v1/drc/review/notes'})]) {
    w.before(d, {}, () => {});
  }
  assert.equal(w.counts.notes, 0); assert.equal(w.counts.waives, 0);
  w.before(request(), {}, () => {}); assert.equal(w.counts.notes, 1);
});
test('original guard denial/redirect is preserved and not counted or held', () => {
  for (const response of [{cancel:true}, {redirectURL:origin+'/blocked'}]) {
    const w = setup(); w.arm('notes'); const received = [];
    w.before(request(), response, value => received.push(value));
    w.headers(request('notes', {statusCode:202}), response, value => received.push(value));
    assert.deepEqual(received, [response, response]);
    assert.equal(w.counts.notes, 0); assert.equal(w.held, null);
  }
});
test('202 is held once after the allowed request, then cancelled once', () => {
  const w = setup(), d = request('notes', {statusCode:202}), calls = [];
  w.arm('notes'); w.before(d, {}, () => {});
  w.headers(d, {}, value => calls.push(value));
  assert.equal(w.held.kind, 'notes'); assert.equal(calls.length, 0);
  w.release(); w.release(); assert.deepEqual(calls, [{cancel:true}]);
  w.before(request('notes', {id:2}), {}, () => {});
  w.headers(request('notes', {id:2,statusCode:202}), {}, value => calls.push(value));
  assert.equal(w.counts.notes, 2); assert.equal(w.held, null); assert.equal(calls.length, 2);
});
test('non-202 or missing request identity fails rather than fabricating acceptance', () => {
  for (const statusCode of [200, 400, 403, 409, 500]) {
    const w = setup(), d = request('notes', {statusCode}); w.arm('notes');
    w.before(d, {}, () => {}); let calls = 0;
    w.headers(d, {}, () => calls++);
    assert.equal(w.failed, true); assert.equal(calls, 1); assert.equal(w.held, null);
  }
  const w = setup(); w.arm('notes');
  w.headers(request('notes', {statusCode:202}), {}, () => {});
  assert.equal(w.failed, true); assert.equal(w.held, null);
});
test('unexpected third POST and duplicate request ids are failures', () => {
  const w = setup(); for (let id=1;id<=3;id++) w.before(request('notes', {id}), {}, () => {});
  assert.equal(w.failed, true);
  const b = setup(); b.before(request(), {}, () => {}); b.before(request(), {}, () => {});
  assert.equal(b.failed, true);
});
test('recovery root and exchange counts remain outside the dead page', () => {
  const w = setup();
  w.before(request('notes', {method:'GET',url:origin+'/'}), {}, () => {});
  w.before(request('notes', {url:origin+'/api/v1/session/exchange'}), {}, () => {});
  const copy = w.snapshot(); copy.roots=99;
  assert.deepEqual(w.snapshot(), {notes:0,waives:0,roots:1,exchanges:1});
});
test('sensitive request/response fields are never accessed', () => {
  const w = setup(), d = request('notes', {statusCode:202});
  for (const key of ['uploadData','requestHeaders','responseHeaders','responseBody','referrer']) {
    Object.defineProperty(d,key,{get(){throw Error('must not read');}});
  }
  w.arm('notes'); w.before(d, {}, () => {}); w.headers(d, {}, () => {}); w.release();
  assert.equal(w.failed, false);
});
test('invalid/repeated arm is refused', () => {
  const w = setup();
  for (const kind of ['roots','__proto__','other',null]) assert.throws(() => w.arm(kind));
  w.arm('notes'); assert.throws(() => w.arm('waives'));
});
test('repair interceptor counts only exact recovery approvals without reading credentials', () => {
  const w=new ReviewIntercept(()=>origin,()=>7,'/recovery');
  for(const kind of ['notes','notes/recovery/prepare','notes/recovery/reconcile'])w.before(request(kind),{},()=>{});
  assert.equal(w.counts.notes,0);
  const d=request('notes/recovery',{statusCode:202});
  for(const key of ['uploadData','requestHeaders','responseHeaders','responseBody'])Object.defineProperty(d,key,{get(){throw Error('sensitive field');}});
  w.arm('notes');w.before(d,{},()=>{});let calls=0;w.headers(d,{},()=>calls++);
  assert.equal(w.held.kind,'notes');w.release();assert.equal(calls,1);assert.equal(w.counts.notes,1);
  assert.throws(()=>new ReviewIntercept(()=>origin,()=>7,'/arbitrary'));
});
