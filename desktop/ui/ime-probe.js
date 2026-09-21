/* Empty-workspace native QA only. Synthetic DOM events, NOT an OS IME test.
 * Return fixed markers; never return paths, typed user text or credentials. */
(function () {
    'use strict';
    const panel=document.getElementById('browse-dialog'),input=document.getElementById('browse-query');
    if(!panel||panel.hidden||!input||input.disabled){return 'ime-failed';}
    const original=input.value;
    input.value='합성 조합';input.focus();
    let passed=true;
    for(const signal of [{isComposing:true,keyCode:13},{isComposing:false,keyCode:229}]){
        for(const key of ['Enter','Escape','Tab']){
            const event=new KeyboardEvent('keydown',Object.assign({key:key,bubbles:true,cancelable:true},signal));
            if(event.isComposing!==signal.isComposing||event.keyCode!==signal.keyCode||
                !input.dispatchEvent(event)||panel.hidden||document.activeElement!==input||input.value!=='합성 조합'){
                passed=false;
            }
        }
    }
    input.value=original;
    return passed?'ime-ok':'ime-failed';
}())
