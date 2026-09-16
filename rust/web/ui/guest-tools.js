/* Private guest annotations. Shared geometry readers and ruler arithmetic are
 * reused, but owner auth, palette editing, export and writes are never bound. */
(function(root){
    'use strict';
    function bind(o){
        const el=function(id){return o.document.getElementById(id);};
        let bound='',active=false,inspector=null,measurement=null,changing=false;
        const wire=o.wire.bind({protocol:o.protocol,context:o.context,send:o.send,
            allowed:function(tool,m){return !(active&&tool==='inspect'&&(m.kind==='snap'||m.body&&m.body.operation&&m.body.operation.kind==='snap'));}});
        const common={document:o.document,window:o.window,protocol:o.protocol,query:o.query,context:o.context,
            now:o.now,setTimeout:o.window.setTimeout.bind(o.window),clearTimeout:o.window.clearTimeout.bind(o.window)};
        inspector=o.inspect.bind(Object.assign({},common,{send:wire.sender('inspect'),layers:o.layers||function(){}}));
        measurement=o.measure.bind(Object.assign({},common,{send:wire.sender('measure'),rulers:o.rulers,
            history:o.history,popCD:o.popCD,cdBusy:o.cdBusy,cdVisible:o.cdVisible,
            selection:function(){return inspector.selection();},modeChanged:function(){
                if(!measurement){return;}
                const next=measurement.active();
                // Retire the old probe BEFORE granting the shared snap slot to
                // the ruler. Later probe checkbox/leave events cannot cancel it.
                if(next&&!active){inspector.interrupt();}
                active=next;if(o.modeChanged){o.modeChanged();}
            }}));
        wire.receiver('inspect',function(m){inspector.receive(m);});
        wire.receiver('measure',function(m){measurement.receive(m);});
        function reset(){
            // Cursor/panel callbacks may call changed(). A teardown must not
            // resume the tools while the caller is retiring its old context.
            const prior=changing;changing=true;
            try{
                bound='';inspector.stop();measurement.stop();wire.reset();active=false;
                ['pick-details','pick-status','snap-status','query-availability','ruler-details','ruler-status','ruler-auto','ruler-availability'].forEach(function(id){el(id).textContent='';});
                el('guest-inspect').hidden=el('guest-rulers').hidden=true;
                if(o.modeChanged){o.modeChanged();}
            }finally{changing=prior;}
        }
        function changed(){
            if(changing){return;}changing=true;
            try{
                const c=o.context(),key=c&&c.connected&&!c.hidden?c.id+':'+c.state.connection_epoch:'';
                if(key!==bound){reset();bound=key;if(key){inspector.resume();measurement.resume();}}
                el('guest-inspect').hidden=el('guest-rulers').hidden=!key;
                if(key){inspector.changed();measurement.changed();}
            }finally{changing=false;}
        }
        function key(k){if(!bound){return false;}return measurement.key(k)||inspector.key(k);}
        reset();
        return {changed:changed,reset:reset,receive:wire.receive,key:key,
            active:function(){return active;},
            leave:function(){measurement.leave();},
            click:function(x,y,m){changed();return bound?(active?measurement.click(x,y,m):inspector.click(x,y,m)):false;},
            move:function(x,y,m){changed();if(bound){if(active){measurement.move(x,y,m);}else{inspector.move(x,y);}}},
            paint:function(p,s){if(bound){inspector.paint(p,s);measurement.paint(p,s);if(!p||!s){inspector.flush();measurement.flush();}}},
            interrupt:function(){inspector.interrupt();measurement.interrupt();},
            selection:function(){return inspector.selection();}};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestTools=api;}
}(typeof window==='object'?window:this));
