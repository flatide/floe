/* Observe a submitted navigation, never send or replay one. */
(function(root){
    'use strict';
    function bind(o){
        const P=o.protocol;let pending=null;
        function identity(a,b){return a&&b&&a.view_id===b.view_id&&a.epoch===b.epoch&&b.mode==='explore';}
        function finish(t,error,value){if(pending!==t){return;}pending=null;o.clearTimeout(t.timer);t.done(error,value);}
        function changed(){const t=pending;if(!t){return;}const c=o.context();
            if(!identity(t.start,c)){finish(t,'View connection changed.');return;}
            if(c.otherInput){finish(t,'New view input superseded the error jump.');return;}
            if(t.accepted===null){return;}
            const relation=P.compare(c.state_rev,t.accepted);
            if(relation<0){return;}
            if(relation>0){finish(t,'The accepted error view was skipped.');return;}
            finish(t,null,{view_id:c.view_id,epoch:c.epoch,state_rev:c.state_rev});
        }
        function begin(c,done){
            if(pending){throw Error('Only one observed error jump');}
            const current=o.context();
            if(!identity(c,current)||c.state_rev!==current.state_rev||current.otherInput){done('View input changed.');return null;}
            P.counter(c.state_rev);
            const t={start:Object.assign({},c),done:done,seq:null,accepted:null,timer:null};pending=t;
            t.timer=o.setTimeout(function(){finish(t,'Error jump unconfirmed; no command was replayed.');},8000);
            return {sent:function(seq){if(pending!==t){return;}if(t.seq!==null){throw Error('Error jump submitted twice');}t.seq=P.counter(seq);},
                cancel:function(){finish(t,'Error jump effects cancelled; a sent move may still complete.');}};
        }
        function accepted(seq,rev){const t=pending;if(!t||t.seq!==seq){return false;}P.counter(rev);
            if(P.compare(rev,t.start.state_rev)<0){finish(t,'Invalid error jump revision.');return true;}
            t.accepted=rev;changed();return true;
        }
        return {begin:begin,accepted:accepted,changed:changed,
            rejected:function(seq){const t=pending;if(t&&t.seq===seq){finish(t,'Error jump rejected.');return true;}return false;},
            reset:function(){if(pending){finish(pending,'Error jump connection closed.');}},busy:function(){return !!pending;}};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestFocusReceipt=api;}
}(typeof window==='object'?window:this));
