/* ES2017 dubeolsik fallback, ported from floe/hangul.py. Local text only:
 * no network, persistence, timer or dependency on a host input-method daemon. */
(function(root) {
    'use strict';
    const KEYMAP={q:'ㅂ',w:'ㅈ',e:'ㄷ',r:'ㄱ',t:'ㅅ',y:'ㅛ',u:'ㅕ',i:'ㅑ',o:'ㅐ',p:'ㅔ',
        a:'ㅁ',s:'ㄴ',d:'ㅇ',f:'ㄹ',g:'ㅎ',h:'ㅗ',j:'ㅓ',k:'ㅏ',l:'ㅣ',
        z:'ㅋ',x:'ㅌ',c:'ㅊ',v:'ㅍ',b:'ㅠ',n:'ㅜ',m:'ㅡ',Q:'ㅃ',W:'ㅉ',E:'ㄸ',R:'ㄲ',T:'ㅆ',O:'ㅒ',P:'ㅖ'};
    const LEADS='ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ', VOWELS='ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ';
    const TAILS='ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ';
    const VC={'ㅗㅏ':'ㅘ','ㅗㅐ':'ㅙ','ㅗㅣ':'ㅚ','ㅜㅓ':'ㅝ','ㅜㅔ':'ㅞ','ㅜㅣ':'ㅟ','ㅡㅣ':'ㅢ'};
    const TC={'ㄱㅅ':'ㄳ','ㄴㅈ':'ㄵ','ㄴㅎ':'ㄶ','ㄹㄱ':'ㄺ','ㄹㅁ':'ㄻ','ㄹㅂ':'ㄼ','ㄹㅅ':'ㄽ',
        'ㄹㅌ':'ㄾ','ㄹㅍ':'ㄿ','ㄹㅎ':'ㅀ','ㅂㅅ':'ㅄ'}, TS={}, VS={};
    Object.keys(TC).forEach(function(k){TS[TC[k]]=k;});
    Object.keys(VC).forEach(function(k){VS[VC[k]]=k[0];});
    class Composer {
        constructor(){this.reset();}
        reset(){this.lead=this.vowel=this.tail='';}
        pending(){return !!(this.lead||this.vowel);}
        preedit(){
            if(!this.vowel){return this.lead;}if(!this.lead){return this.vowel;}
            return String.fromCharCode(0xac00+(LEADS.indexOf(this.lead)*21+VOWELS.indexOf(this.vowel))*28+
                (this.tail?TAILS.indexOf(this.tail)+1:0));
        }
        feed(jamo){
            if(LEADS.includes(jamo)){
                if(!this.lead&&!this.vowel){this.lead=jamo;return ['',this.preedit()];}
                if(this.lead&&!this.vowel){const out=this.preedit();this.lead=jamo;return [out,this.preedit()];}
                if(this.lead&&!this.tail&&TAILS.includes(jamo)){this.tail=jamo;return ['',this.preedit()];}
                const combo=TC[this.tail+jamo];
                if(this.tail&&combo){this.tail=combo;return ['',this.preedit()];}
                const out=this.preedit();this.reset();this.lead=jamo;return [out,this.preedit()];
            }
            if(this.tail){
                const split=TS[this.tail],move=split?split[1]:this.tail;
                this.tail=split?split[0]:'';const out=this.preedit();
                this.lead=move;this.vowel=jamo;this.tail='';return [out,this.preedit()];
            }
            if(this.vowel){
                const combo=VC[this.vowel+jamo];
                if(combo){this.vowel=combo;return ['',this.preedit()];}
                const out=this.preedit();this.reset();this.vowel=jamo;return [out,this.preedit()];
            }
            this.vowel=jamo;return ['',this.preedit()];
        }
        backspace(){
            if(this.tail){this.tail=TS[this.tail]?TS[this.tail][0]:'';}
            else if(this.vowel){this.vowel=VS[this.vowel]||'';}else{this.lead='';}
            return this.preedit();
        }
    }
    function bind(o){
        const input=o.input,toggle=o.toggle,hint=o.hint,c=new Composer();
        const supported=typeof input.setRangeText==='function';
        let anchor=null,allowed=false,native=false,limited=false;
        toggle.checked=false;
        function reset(){c.reset();anchor=null;limited=false;}
        function composing(e){return native||!!e.isComposing||e.keyCode===229;}
        function paint(){
            toggle.disabled=!supported||!allowed;
            const message=!supported?'Built-in Hangul unavailable; use a browser/OS IME.':
                limited?'Text length limit reached. Finish or shorten the note.':
                'Built-in Hangul '+(toggle.checked?'on':'off')+' · Shift+Space toggles · OS IME takes priority';
            // Do not reannounce an unchanged live-region hint for every jamo.
            if(hint.textContent!==message){hint.textContent=message;}
        }
        function external(){reset();paint();}
        function key(e){
            if(!supported||!allowed||input.disabled||input.readOnly){reset();return false;}
            if(composing(e)){reset();return false;}
            if(e.ctrlKey||e.altKey||e.metaKey){reset();return false;}
            if(['HangulMode','HanjaMode','Hangul','Hangul_Hanja'].includes(e.key)||(e.key===' '&&e.shiftKey)){
                reset();toggle.checked=!toggle.checked;paint();return true;
            }
            if(!toggle.checked){return false;}
            // Text-control offsets are UTF-16 units (including emoji prefixes).
            // A cursor move, selection or external edit commits the old preedit.
            if(c.pending()&&(input.selectionStart!==anchor+c.preedit().length||
                input.selectionEnd!==input.selectionStart||input.value.slice(anchor,input.selectionStart)!==c.preedit())){reset();}
            const pending=c.pending(),old=c.preedit(),state=[c.lead,c.vowel,c.tail];
            let start=pending?anchor:input.selectionStart,end=pending?anchor+old.length:input.selectionEnd,committed='',preedit;
            if(e.key==='Backspace'&&pending){preedit=c.backspace();}
            else {
                const key=typeof e.key==='string'?e.key:'';
                const jamo=key.length===1&&(KEYMAP[key]||KEYMAP[key.toLowerCase()]);
                if(!jamo){if(key!=='Shift'&&key!=='CapsLock'){external();}return false;}
                const result=c.feed(jamo);committed=result[0];preedit=result[1];
            }
            const replacement=committed+preedit;
            // setRangeText bypasses the native maxlength insertion guard.
            if(input.maxLength>=0&&input.value.length-(end-start)+replacement.length>input.maxLength){
                c.lead=state[0];c.vowel=state[1];c.tail=state[2];limited=true;paint();return true;
            }
            input.setRangeText(replacement,start,end,'end');
            anchor=c.pending()?start+committed.length:null;limited=false;paint();o.changed();return true;
        }
        input.onkeydown=function(e){if(key(e)){e.preventDefault();e.stopPropagation();}};
        input.oninput=function(){external();o.changed();};
        input.oncompositionstart=function(){native=true;external();};
        input.oncompositionend=function(){native=false;external();};
        input.onblur=input.onpaste=input.oncut=input.ondrop=external;
        toggle.onchange=function(){reset();if(!allowed||!supported){toggle.checked=false;}paint();if(allowed){input.focus();}};
        paint();
        return {composing:composing,reset:external,
            sync:function(value){allowed=value===true;if(!allowed){reset();}paint();},
            close:function(){native=false;toggle.checked=false;external();}};
    }
    const api={Composer:Composer,KEYMAP:KEYMAP,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeHangul=api;}
}(typeof window==='object'?window:this));
