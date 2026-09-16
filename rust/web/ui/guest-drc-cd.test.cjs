'use strict';
const assert=require('node:assert/strict'),{environment,tick}=require('./guest-drc.test.cjs');
const reads=(e,kind)=>e.calls.filter(c=>c.path==='/drc/read'&&c.body.body.kind===kind);
const panelWrites=e=>e.calls.filter(c=>c.path==='/drc/panel'&&c.method==='POST');
const cds=e=>e.history.entries().filter(v=>v.kind==='cd');
async function hold(h){for(let i=0;i<40&&!h.resolve;i++){await tick();}assert(h.resolve,'expected pending operation');}
async function selected(){const e=environment();e.panelUI.changed();await e.settle();e.el('gd-errors').children[0].onclick({});await e.settle();return e;}
(async()=>{
    const e=await selected(),nav=e.holdNavigation();e.el('gd-go').onclick();await hold(nav);
    assert.equal(reads(e,'measurements').length,0);assert.equal(e.panel().jump_active,false);assert.equal(cds(e).length,0);
    nav.resolve();await e.settle();assert.equal(cds(e).length,2);assert.equal(cds(e)[0].value.ends[0][0],10.000125);
    assert.equal(e.panel().cd.target.error,e.a.local);assert.equal(e.panel().jump_active,true);assert(e.panel().zoom_lock,'Go keeps the current scale');
    e.paint();e.panelUI.click(50,12,false,{});await e.settle();assert.equal(e.panel().selected.error,e.b.local);
    assert.equal(e.panel().cd.target.error,e.a.local);assert.equal(e.moves.length,1,'plain marker selection never jumps');
    e.panelUI.move(50,12);assert.match(e.hover(),/waived/);e.panelUI.move(NaN,NaN);assert.equal(e.hover(),'');
    e.history.push('manual',{manual:'kept'});e.el('gd-reload').onclick();await e.settle();assert.equal(e.moves.length,1,'restore must not repeat a move');
    assert.equal(reads(e,'measurements').at(-1).body.body.error,e.a.local);assert.equal(e.panel().selected.error,e.b.local);
    assert(e.history.entries().some(v=>v.kind==='manual'),'review reload preserves manual rulers');
    e.el('gd-markers').checked=false;e.el('gd-markers').onchange();await e.settle();assert.equal(e.panelUI.visible(),false);assert.equal(cds(e).length,2);
    e.el('gd-markers').checked=true;e.el('gd-markers').onchange();await e.settle();assert(e.panelUI.visible());
    assert(e.panelUI.popCD(false));await e.settle();assert.equal(cds(e).length,1);assert.equal(e.panel().cd.remaining,1);
    assert(e.panelUI.key('Escape'));await e.settle();assert.equal(cds(e).length,0);assert(e.panel().jump_active);
    assert(e.panelUI.key('Escape'));await e.settle();assert.equal(e.panel().jump_active,false);
    const count=e.moves.length;e.el('gd-step-next').onclick();await e.settle();assert.equal(e.moves.length,count,'end navigation leaves selection-only stepping');
    e.el('gd-fit').onclick();await e.settle();assert.equal(e.panel().zoom_lock,false);
    e.el('gd-step-next').onclick();await e.settle();assert.equal(reads(e,'focus').at(-1).body.body.fit,true,'auto-fit follows confirmed jump mode');
    e.context({...e.getContext(),camera:['32','16','128'],state_rev:'2'});e.panelUI.changed();e.el('gd-step-next').onclick();await e.settle();
    assert.equal(reads(e,'focus').at(-1).body.body.fit,false);assert.equal(e.panel().zoom_lock,true,'manual zoom locks following error jumps');
    e.el('gd-fit').onclick();await e.settle();assert.equal(e.panel().zoom_lock,false,'explicit Frame error unlocks auto-fit');
    // Pending CD reserves creation order, then resolves without overtaking a
    // newer manual annotation. No CD read is made for an ordinary pan.
    const order=await selected();order.history.push('manual',{id:'before'});const measurement=order.hold('measurements');order.el('gd-go').onclick();await hold(measurement);
    assert.equal(cds(order).length,3);assert(cds(order).every(v=>v.value===null));order.history.push('manual',{id:'after'});
    measurement.resolve();await order.settle();assert.deepEqual(order.history.entries().map(v=>v.kind),['manual','cd','cd','manual']);
    const measured=reads(order,'measurements').length;order.context({...order.getContext(),state_rev:'2'});order.panelUI.changed();await order.settle();assert.equal(reads(order,'measurements').length,measured);
    // Removing a pending group retires its late reply; a pending replacement
    // move likewise cannot resurrect an old or new group.
    const pending=await selected(),slow=pending.hold('measurements');pending.el('gd-go').onclick();await hold(slow);assert(pending.panelUI.popCD(false));
    slow.resolve();await pending.settle();assert.equal(cds(pending).length,0);assert.equal(pending.panel().cd.remaining,0);
    pending.el('gd-go').onclick();await pending.settle();const replacement=pending.holdNavigation();pending.el('gd-fit').onclick();await hold(replacement);
    assert(pending.panelUI.popCD(true));await pending.settle();replacement.resolve();await tick();assert.equal(cds(pending).length,0);assert.match(pending.el('gd-status').textContent,/sent move/);
    // Deleting while a panel write is in flight is a NEW intent after a known
    // CAS receipt, not a retry. An uncertain receipt must stop all followups.
    const conflict=await selected(),saving=conflict.hold('panel');conflict.el('gd-go').onclick();await hold(saving);
    assert.equal(cds(conflict).length,3);conflict.failPanel();const writes=panelWrites(conflict).length;assert(conflict.panelUI.popCD(true));saving.resolve();
    for(let i=0;i<30;i++){await tick();}assert(conflict.busy());assert.match(conflict.el('gd-status').textContent,/unconfirmed/);assert.equal(panelWrites(conflict).length,writes,'never retry uncertain save');
    conflict.el('gd-reload').onclick();await conflict.settle();assert.equal(conflict.moves.length,1,'reconciliation does not navigate');
    const confirmed=await selected(),saving2=confirmed.hold('panel');confirmed.el('gd-go').onclick();await hold(saving2);assert(confirmed.panelUI.popCD(true));saving2.resolve();await confirmed.settle();
    assert.equal(confirmed.panel().cd.remaining,0);assert.equal(cds(confirmed).length,0,'confirmed earlier save is followed by the explicit deletion intent');
    // A current-view filter remains active until a successful jump receipt.
    const filtered=await selected();filtered.el('gd-in-view').checked=true;filtered.el('gd-in-view').onchange();await filtered.settle();
    const rejected=filtered.holdNavigation();filtered.el('gd-go').onclick();await hold(rejected);assert(filtered.panel().in_view);rejected.resolve('rejected');await filtered.settle();assert(filtered.panel().in_view);
    filtered.el('gd-go').onclick();await filtered.settle();assert.equal(filtered.panel().in_view,false);
    const revoked=await selected(),late=revoked.hold('measurements');revoked.el('gd-go').onclick();await hold(late);revoked.context(null);revoked.panelUI.changed();late.resolve();await tick();assert.equal(cds(revoked).length,0);assert(revoked.el('gd-panel').hidden);
    const follow=environment('follow');follow.panelUI.changed();await follow.settle();follow.el('gd-errors').children[0].onclick({});await follow.settle();follow.el('gd-go').onclick();await follow.settle();assert.equal(reads(follow,'measurements').length,0);assert.equal(follow.moves.length,0);
    console.log('GUEST DRC CD: ALL OK (confirmed target, independent selection/restore, auto-fit/zoom lock, history order, delayed delete/cancel, CAS loss/no replay, follow/revoke)');
})().catch(e=>{console.error(e);process.exitCode=1;});
