/* Reuse read-only inspectors without giving them the owner dispatcher.
 * Replies are routed by locally sent sequence, not whichever tool is active. */
(function(root){
    'use strict';
    const commands={'view.query':'explore.query','view.query.cancel':'explore.query.cancel',
        'view.measure':'explore.measure','view.measure_selection':'explore.measure_selection'};
    const replies=['query.accepted','query.result','query.cancelled','measure.result','measure_selection.result'];
    function bind(o){
        const sent=new Map(),order=[],receivers={inspect:null,measure:null};
        function sender(tool){
            if(!Object.prototype.hasOwnProperty.call(receivers,tool)){throw Error('Unknown guest tool');}
            return function(m){
                const c=o.context(),type=m&&typeof m.type==='string'&&Object.prototype.hasOwnProperty.call(commands,m.type)?commands[m.type]:null;
                if(!type||!c||!c.connected||m.view_id!==c.id||m.connection_epoch!==c.state.connection_epoch){throw Error('Guest query is not connected');}
                if(tool==='inspect'&&['view.measure','view.measure_selection'].includes(m.type)||
                    tool==='measure'&&m.type==='view.query'&&(!m.body||!m.body.operation||m.body.operation.kind!=='snap')){throw Error('Wrong guest query tool');}
                if(o.allowed&&!o.allowed(tool,m)){throw Error('Another tool owns this query');}
                const seq=o.protocol.counter(o.send(Object.assign({},m,{type:type})));
                const expected=m.type==='view.query'?['query.accepted','query.result']:m.type==='view.query.cancel'?['query.cancelled']:
                    m.type==='view.measure'?['measure.result']:['measure_selection.result'];
                sent.set(seq,{tool:tool,expected:expected,done:false});order.push(seq);
                // Late error envelopes belong to retired requests, not current
                // navigation. Bounded history contains IDs only, no geometry.
                if(order.length>512){sent.delete(order.shift());}
                return seq;
            };
        }
        function receive(m){
            if(!m||m.type!=='error'&&!replies.includes(m.type)){return false;}
            const entry=sent.get(m.seq);
            if(!entry){return m.type!=='error';}
            if(entry.done){return true;}
            if(m.type!=='error'&&!entry.expected.includes(m.type)){throw Error('Wrong guest query reply');}
            if(m.type!=='query.accepted'){entry.done=true;}
            const receiver=receivers[entry.tool];if(receiver){receiver(m);}return true;
        }
        return {sender:sender,receive:receive,
            receiver:function(tool,fn){if(!Object.prototype.hasOwnProperty.call(receivers,tool)||typeof fn!=='function'){throw Error('Unknown guest receiver');}receivers[tool]=fn;},
            reset:function(){sent.clear();order.length=0;}};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestQueryWire=api;}
}(typeof window==='object'?window:this));
