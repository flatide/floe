'use strict';
// Deterministic text-control adapter, not a real-browser keyboard/IME test.
const assert=require('node:assert/strict'),fs=require('node:fs'),H=require('./hangul.js');
function harness(text=''){
    let value=text,fullWrites=0,changes=0,statusWrites=0,status='';
    const input={selectionStart:text.length,selectionEnd:text.length,maxLength:65536,disabled:false,readOnly:false,scrollTop:300,
        get value(){return value;},set value(v){++fullWrites;value=v;},focus(){},
        setRangeText(v,start,end,mode){assert.equal(mode,'end');value=value.slice(0,start)+v+value.slice(end);this.selectionStart=this.selectionEnd=start+v.length;}};
    const toggle={checked:false,disabled:false},hint={get textContent(){return status;},set textContent(v){status=v;++statusWrites;}};
    const adapter=H.bind({input,toggle,hint,changed(){++changes;}});adapter.sync(true);
    function key(key,extra={}){const e={key,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;},...extra};input.onkeydown(e);assert.equal(e.prevented,e.stopped);return !!e.prevented;}
    function type(keys){for(const k of keys)assert(key(k),'unhandled '+k);}
    return {input,toggle,hint,adapter,key,type,get fullWrites(){return fullWrites;},get changes(){return changes;},get statusWrites(){return statusWrites;}};
}
function test(){
    const h=harness('😀 prefix ');assert(!h.key('g'));assert(h.key(' ',{shiftKey:true}));h.type('gksrmf');
    assert.equal(h.input.value,'😀 prefix 한글');assert.equal(h.fullWrites,0);assert.equal(h.input.scrollTop,300);assert.equal(h.statusWrites,2);
    for(const [keys,expected] of [['rhk','과'],['rnj','궈'],['rkqt','값'],['rkqtk','갑사'],['Rk','까'],['Tkd','쌍'],['dkssudgktpdy','안녕하세요']]){
        const c=harness();c.key('HangulMode');c.type(keys);assert.equal(c.input.value,expected,keys);
    }
    const b=harness();b.key('HanjaMode');b.type('rhkrt');assert.equal(b.input.value,'곿');
    for(const value of ['곽','과','고','ㄱ','']){assert(b.key('Backspace'));assert.equal(b.input.value,value);}
    assert(!b.key('Backspace'));b.type('r');assert(!b.key('Shift'));b.type('K');assert.equal(b.input.value,'가');
    // Native text is never replaced by a stale preedit at an old caret/selection.
    const s=harness('😀abc');s.key(' ',{shiftKey:true});s.input.selectionStart=2;s.input.selectionEnd=5;s.type('rk');assert.equal(s.input.value,'😀가');
    s.input.selectionStart=s.input.selectionEnd=0;s.type('s');assert.equal(s.input.value,'ㄴ😀가');
    s.input.value='changed';s.input.selectionStart=s.input.selectionEnd=7;s.type('r');assert.equal(s.input.value,'changedㄱ');
    s.input.onpaste();s.type('k');assert.equal(s.input.value,'changedㄱㅏ');
    s.input.oncut();s.type('r');s.input.ondrop();s.type('k');assert(s.input.value.endsWith('ㄱㅏ'));
    for(const extra of [{ctrlKey:true},{metaKey:true},{altKey:true}]){
        const c=harness();c.key('HangulMode');c.type('r');assert(!c.key('z',extra));c.type('k');assert.equal(c.input.value,'ㄱㅏ');
        assert(!c.key(' ',{shiftKey:true,...extra}));assert(c.toggle.checked);
    }
    const n=harness();n.key('HangulMode');n.type('r');n.input.oncompositionstart();
    assert(!n.key('k'));assert(n.adapter.composing({key:'Enter'}));n.input.value='한';n.input.selectionStart=n.input.selectionEnd=1;n.input.oninput();
    n.input.oncompositionend();assert(!n.adapter.composing({key:'Enter'}));n.type('r');assert.equal(n.input.value,'한ㄱ');
    assert(!n.key('k',{keyCode:229}));n.type('k');assert.equal(n.input.value,'한ㄱㅏ');assert(!n.key('x',{isComposing:true}));
    const cap=harness();cap.input.maxLength=1;cap.key('HangulMode');cap.type('rk');assert.equal(cap.input.value,'가');
    cap.type('s');assert.equal(cap.input.value,'간');cap.type('k');assert.equal(cap.input.value,'간');assert.match(cap.hint.textContent,/limit/);
    cap.key('Backspace');assert.equal(cap.input.value,'가');cap.type('t');assert.equal(cap.input.value,'갓');
    cap.input.maxLength=0;cap.input.selectionStart=0;cap.input.selectionEnd=1;cap.type('r');assert.equal(cap.input.value,'갓');
    const off=harness();off.key('HangulMode');off.type('r');off.adapter.sync(false);assert(off.toggle.disabled);assert(!off.key('k'));
    off.adapter.sync(true);off.type('k');assert.equal(off.input.value,'ㄱㅏ');off.input.onblur();off.adapter.close();assert(!off.toggle.checked);
    off.key('HangulMode');off.input.readOnly=true;assert(!off.key('r'));off.input.readOnly=false;off.input.disabled=true;assert(!off.key('r'));
    const unsupported={input:{value:'',focus(){}},toggle:{},hint:{}};
    const u=H.bind({...unsupported,changed(){}});u.sync(true);assert(unsupported.toggle.disabled);assert.match(unsupported.hint.textContent,/unavailable/);
    const html=fs.readFileSync(__dirname+'/index.html','utf8');assert(html.indexOf('/hangul.js')>=0);assert(html.indexOf('/hangul.js')<html.indexOf('/drc-notes.js'));
    console.log('WEB HANGUL: ALL OK (composition, caret/selection/UTF16, external edits, IME/shortcuts, length cap, lifecycle, local span updates)');
}
function oracle(v){
    assert.deepEqual(H.KEYMAP,v.keymap);let steps=0;
    for(const rows of v.cases){const c=new H.Composer();for(const [key,committed,preedit,pending] of rows){
        let out='';if(key==='reset')c.reset();else if(key==='backspace')c.backspace();else out=c.feed(key)[0];
        assert.deepEqual([out,c.preedit(),c.pending()],[committed,preedit,pending],JSON.stringify(rows));++steps;
    }}
    console.log(`GTK/WEB HANGUL ORACLE: ALL OK (${v.syllables} syllables, ${v.cases.length} sequences, ${steps} transitions)`);
}
if(process.argv[2]==='--oracle')oracle(JSON.parse(fs.readFileSync(0,'utf8')));else test();
