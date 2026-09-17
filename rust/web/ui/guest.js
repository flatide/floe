/* ES2017. A separate guest application: never initializes owner auth or writes. */
(function(root){
    'use strict';
    function bind(o){
        const win=o.window,doc=o.document,P=o.protocol,el=function(id){return doc.getElementById(id);};
        const bundle=doc.querySelector('meta[name="floe-bundle"]').content;
        const match=/^\/guest\/([0-9a-f]{64})$/.exec(o.location.pathname),id=match?match[1]:'';
        const base='/api/v1/guest/'+id,key='floe-guest-session:'+o.location.origin+':'+id;
        const canvas=el('guest-canvas'),viewport=el('guest-viewport'),ctx=canvas.getContext('2d',{alpha:false});
        const drcCanvas=el('guest-drc-canvas'),drcContext=drcCanvas.getContext('2d');
        const edits=['fit','in','out','depth','detail','thin','frames','labels','mono','x','y','width','go'];
        let auth=null,session=null,socket=null,hello=null,state=null,seq='0',serial=0;
        let started=false,stopped=false,suspended=false,pageSuspended=false,joining=false,decode=null,raf=null;
        let foreground=null,margin=null,flight=null,accepted=null,queue=[];
        let ackedFrames={foreground:null,margin:null},displayed=false,queryTools=null,gesture=null,dragShift=null,overlaySize='';
        let ping=null,reconnect=null,resize=null,observer=null,delay=500,flushTimer=null,lastSend=-Infinity;
        let focusTicket=null,overlayRAF=null;
        const rulerHistory=o.rulers?o.rulers.history():null;
        const now=o.now||function(){return win.performance.now();};
        const pending=new Set();let exchanging=null;
        // An invitation is single-use. Keep only its already submitted, bounded
        // exchange alive across hiding; never replay it or start follow-up work.
        function abortReads(){pending.forEach(function(x){if(x!==exchanging){x.abort();}});}
        function inputPending(ignoreGesture){return !!flight||!!accepted||!!queue.length||!ignoreGesture&&(!!dragShift||!!gesture&&gesture.active());}
        function queryContext(ignoreGesture){return o.display?o.display.context({protocol:P,session:session,hello:hello,state:state,
            foreground:foreground&&foreground.header,margin:margin&&margin.header,acked:ackedFrames,
            canvasRect:canvas.getBoundingClientRect(),viewportRect:viewport.getBoundingClientRect(),
            connected:!stopped&&!suspended&&!!socket&&socket.readyState===1,hidden:!!doc.hidden,
            pending:inputPending(true)||!ignoreGesture&&(!!dragShift||!!gesture&&gesture.moving())}):null;}
        function viewContext(){return !stopped&&!suspended&&hello&&state?{view_id:hello.view_id,epoch:hello.connection_epoch,
            state_rev:state.state_rev,render_key:state.render_key,mode:session.mode,pending:inputPending()}:null;}
        const focusReceipt=o.focusReceipt?o.focusReceipt.bind({protocol:P,setTimeout:win.setTimeout.bind(win),clearTimeout:win.clearTimeout.bind(win),
            context:function(){const c=viewContext();return c&&Object.assign({},c,{otherInput:queue.some(function(q){return q.notice!==focusTicket;})||!!dragShift||!!gesture&&gesture.moving()});}}):null;
        function errorNavigation(n,c,done){const current=viewContext();if(!focusReceipt||!current||current.pending||current.mode!=='explore'||
            c.view_id!==current.view_id||c.epoch!==current.epoch||c.state_rev!==current.state_rev){done('View input changed; select Go again.');return function(){};}
            let ticket=null;ticket=focusReceipt.begin(c,function(error,receipt){
                if(error){queue=queue.filter(function(q){return q.notice!==ticket;});}
                if(focusTicket===ticket){focusTicket=null;}done(error,receipt);flush();});
            if(!ticket){return function(){};}focusTicket=ticket;edit({navigation:n},ticket);return ticket.cancel;
        }
        function scopedEdit(body,c){const current=viewContext();if(!current||current.mode!=='explore'||current.pending||
            c.view_id!==current.view_id||c.epoch!==current.epoch||c.state_rev!==current.state_rev){return false;}edit(body);return true;}
        const layers=o.layers?o.layers.bind({el:el,document:doc,protocol:P,http:request,context:viewContext,edit:scopedEdit}):null;
        function panelsChanged(){if(drc){drc.changed();}if(layers){layers.changed();}if(queryTools){queryTools.changed();}}
        const drc=o.drc?o.drc.bind({el:el,document:doc,protocol:P,geometry:o.geometry,selection:o.selection,steps:o.drcSteps,http:request,rulers:o.rulers,history:rulerHistory,
            rect:function(){return drcCanvas.getBoundingClientRect();},
            hover:function(text){viewport.title=text;},
            context:function(){return !stopped&&!suspended&&hello&&state&&session.drc?{view_id:hello.view_id,epoch:hello.connection_epoch,
                state_rev:state.state_rev,mode:session.mode,drc:session.drc,camera:state.camera_um,pixels:state.pixels,pending:inputPending(true)||!!dragShift||!!gesture&&gesture.moving()}:null;},
            modeChanged:function(active){if(active&&queryTools&&queryTools.active()){queryTools.leave();}if(active&&gesture){gesture.cancel();}
                viewport.style.cursor=active?'crosshair':'';},
            repaint:overlayLater,navigate:errorNavigation}):null;
        if(o.tools){queryTools=o.tools.bind({window:win,document:doc,protocol:P,query:o.query,wire:o.queryWire,inspect:o.inspect,measure:o.measure,rulers:o.rulers,
            history:rulerHistory,popCD:function(all){return drc&&drc.popCD(all);},cdBusy:function(){return !!drc&&drc.restoring();},cdVisible:function(){return !!drc&&drc.visible();},
            context:function(){return queryContext(false);},now:now,send:function(v){
                if(!socket||socket.bufferedAmount>16384||new TextEncoder().encode(JSON.stringify(Object.assign({},v,{seq:P.next(seq)}))).length>8192){throw Error('Guest query input limit');}return send(v);},
            layers:function(pairs){if(layers&&layers.highlight){layers.highlight(pairs);}},
            modeChanged:function(){if(queryTools&&queryTools.active()&&drc){drc.leave();}if(gesture){gesture.cancel();}cursor();}});}
        function cursor(){viewport.style.cursor=gesture&&gesture.bandActive()?'crosshair':gesture&&gesture.active()?'grabbing':queryTools&&queryTools.active()||drc&&drc.active()?'crosshair':'';panelsChanged();}
        function overlayLater(){if(!state||stopped||suspended||overlayRAF!==null){return;}overlayRAF=win.requestAnimationFrame(function(){overlayRAF=null;paintOverlays(false);});}
        function paintOverlays(resizeOnly){let size=null,projection=null;
            if(overlayRAF!==null){win.cancelAnimationFrame(overlayRAF);overlayRAF=null;}
            if(state&&displayed){try{size=o.display.size(state.pixels,canvas.getBoundingClientRect(),viewport.getBoundingClientRect());
                projection=o.geometry.projection({bbox_dbu:state.bbox_dbu,width:state.pixels[0],height:state.pixels[1]},dragShift||[0,0],state.dbu_um);}catch(_){size=null;}}
            const key=JSON.stringify(size);if(resizeOnly&&key===overlaySize){return;}overlaySize=key;
            drcCanvas.hidden=!drc||!session||!session.drc||!projection||!size;
            if(!drcCanvas.hidden){const w=size.pixels[0],h=size.pixels[1];if(drcCanvas.width!==w||drcCanvas.height!==h){drcCanvas.width=w;drcCanvas.height=h;}
                drcCanvas.style.width=w/size.dpr+'px';drcCanvas.style.height=h/size.dpr+'px';drcCanvas.style.left=size.left+'px';drcCanvas.style.top=size.top+'px';
                drcContext.clearRect(0,0,w,h);drc.paint(drcContext,projection,state.pixels,drcCanvas.getBoundingClientRect());}
            else if(drc){drc.paint(null,null,null,null);}
            if(queryTools){queryTools.paint(projection,size);}}
        function status(s){el('guest-status').textContent=s;}
        function remove(){try{win.sessionStorage.removeItem(key);}catch(_){/* no persistent fallback */}}
        function valid(a){return a&&a.protocol===1&&a.bundle===bundle&&a.share_id===id&&
            ['session_id','csrf'].every(function(k){return typeof a[k]==='string'&&/^[0-9a-f]{64}$/.test(a[k]);});}
        function controls(){const live=!stopped&&!suspended&&!!hello&&!!state;
            el('guest-controls').hidden=!session||session.mode!=='explore';
            edits.forEach(function(n){el('guest-'+n).disabled=!live||session.mode!=='explore';});
            el('guest-leave').disabled=!auth||stopped;el('guest-reconnect').disabled=!auth||stopped||joining||!!socket;
        }
        function clear(){foreground=margin=null;ackedFrames={foreground:null,margin:null};displayed=false;canvas.width=canvas.height=1;
            overlaySize='';drcCanvas.hidden=true;drcCanvas.width=drcCanvas.height=1;
            if(queryTools){queryTools.paint(null,null);}el('guest-empty').hidden=false;el('guest-frame-status').textContent='No displayed frame';}
        function disconnect(){serial++;joining=false;const old=socket;socket=null;hello=state=null;queue=[];flight=accepted=null;
            if(focusReceipt){focusReceipt.reset();}focusTicket=null;
            if(overlayRAF!==null){win.cancelAnimationFrame(overlayRAF);overlayRAF=null;}
            if(gesture){gesture.cancel();}dragShift=null;if(queryTools){queryTools.reset();}
            if(drc){drc.reset();}
            if(layers){layers.reset();}
            if(flushTimer!==null){win.clearTimeout(flushTimer);flushTimer=null;}lastSend=-Infinity;
            if(decode){decode.cancel();decode=null;}if(raf!==null){win.cancelAnimationFrame(raf);raf=null;}
            if(ping!==null){win.clearInterval(ping);ping=null;}if(reconnect!==null){win.clearTimeout(reconnect);reconnect=null;}
            if(resize!==null){win.clearTimeout(resize);resize=null;}if(old){old.onclose=old.onmessage=old.onerror=null;old.close();}clear();controls();
        }
        function stop(message,forget){stopped=true;disconnect();pending.forEach(function(x){x.abort();});if(observer){observer.disconnect();}if(forget){remove();auth=null;}status(message);controls();}
        function request(method,suffix,body,token){
            // This client cannot dispatch owner paths, even via a caller option.
            const isDRC=['/drc','/drc/read','/drc/panel','/drc/selection'].includes(suffix);
            const methods={'/exchange':['POST'],'/session':['GET','DELETE'],'/layers':['POST'],'/drc':['GET'],'/drc/read':['POST'],'/drc/panel':['GET','POST'],'/drc/selection':['GET','POST']};
            if(!methods[suffix]||!methods[suffix].includes(method)||isDRC&&(!session||!session.drc||!hello||!state)){return Promise.reject(Error('Unsupported guest request'));}
            if(suffix==='/layers'&&(!hello||!state)){return Promise.reject(Error('Guest view not ready'));}
            const limit=isDRC?1024*1024:suffix==='/layers'?256*1024:65536;
            return new Promise(function(resolve,reject){const x=new o.XHR();pending.add(x);if(suffix==='/exchange'){exchanging=x;}let done=false;
                function end(error,value){if(done){return;}done=true;pending.delete(x);if(exchanging===x){exchanging=null;}if(token){token.abort=null;}if(error){reject(error);}else{resolve(value);}}
                if(token&&token.cancelled){end(Error('Guest request cancelled'));return;}
                if(token){token.abort=function(){x.abort();};}
                x.open(method,base+suffix,true);x.timeout=isDRC?30000:8000;
                if(auth){x.setRequestHeader('X-Floe-Guest-CSRF',auth.csrf);}if(body!==undefined){x.setRequestHeader('Content-Type','application/json');}
                x.onprogress=function(e){if(e.loaded>limit||e.lengthComputable&&e.total>limit){end(Error('Guest reply limit'));x.abort();}};
                x.onload=function(){if(x.status<200||x.status>=300){const e=Error('Guest access unavailable (HTTP '+x.status+').');e.status=x.status;end(e);return;}
                    try{if(x.responseText.length>limit){throw Error('Guest reply limit');}end(null,x.responseText?JSON.parse(x.responseText):null);}catch(_){end(Error('Invalid guest reply'));}};
                x.onerror=x.ontimeout=function(){end(Error('Local service unavailable; no command was replayed.'));};
                x.onabort=function(){end(Error('Guest request cancelled'));};x.send(body===undefined?null:JSON.stringify(body));
            });
        }
        function send(v){if(!socket||socket.readyState!==1||!hello){throw Error('Guest is disconnected');}
            seq=P.next(seq);v.seq=seq;socket.send(JSON.stringify(v));return seq;}
        function ack(h,shown){send({type:'frame.ack',connection_epoch:hello.connection_epoch,frame_id:h.frame_id,disposition:shown?'displayed':'discarded'});
            if(shown){ackedFrames[h.purpose]=h.frame_id;}if(queryTools){queryTools.changed();}}
        function place(f){if(!f||!state){return null;}if(P.matches(f.header,state)&&f.header.purpose==='foreground'){return [0,0];}return P.placement(f.header,state);}
        function compose(){if(!state){return false;}canvas.width=state.pixels[0];canvas.height=state.pixels[1];ctx.imageSmoothingEnabled=false;ctx.fillStyle='#000';ctx.fillRect(0,0,canvas.width,canvas.height);
            const shift=dragShift||[0,0];let shown=false;[margin,foreground].forEach(function(f){const p=place(f);if(p&&p[0]+shift[0]<f.header.width&&p[1]+shift[1]<f.header.height&&p[0]+shift[0]+canvas.width>0&&p[1]+shift[1]+canvas.height>0){ctx.drawImage(f.canvas,-p[0]-shift[0],-p[1]-shift[1]);shown=true;}});
            el('guest-empty').hidden=shown;
            displayed=shown;paintOverlays(false);
            return shown;
        }
        function frame(buffer,token){const packet=P.packet(buffer),h=packet.header;
            if(!hello||h.view_id!==hello.view_id||h.connection_epoch!==hello.connection_epoch){throw Error('Wrong guest frame identity');}
            if(decode||raf!==null){throw Error('Guest frame credit exceeded');}
            if(!P.matches(h,state)){ack(h,false);return;}
            const image=doc.createElement('canvas');image.width=h.width;image.height=h.height;
            const job=o.decode(h,packet.data,function(draw,error){decode=null;if(token!==serial||stopped||suspended){return;}
                if(error){ack(h,false);status('Frame decode failed; reconnect to retry.');return;}
                if(!draw||!P.matches(h,state)){ack(h,false);return;}
                try{draw(image.getContext('2d',{alpha:false}));}catch(_){ack(h,false);status('Canvas decode failed; reconnect to retry.');return;}
                raf=win.requestAnimationFrame(function(){raf=null;if(token!==serial||stopped||suspended){return;}
                    if(!socket||socket.readyState!==1){return;}
                    if(!P.matches(h,state)){ack(h,false);return;}
                    const f={header:h,canvas:image};if(h.purpose==='margin'){margin=f;}else{foreground=f;}
                    let shown=false;try{shown=compose();}catch(_){clear();ack(h,false);status('Canvas presentation failed; reconnect to retry.');return;}ack(h,shown);
                    el('guest-frame-status').textContent=(h.complete?'Complete':'Partial / incomplete')+(h.approximate?' · approximate':'')+' · '+h.width+' × '+h.height+' px · '+h.format+(h.purpose==='margin'?' · margin':'');
                });
            });decode=job;job.start();
        }
        function validateState(s){if(!hello||s.view_id!==hello.view_id||s.connection_epoch!==hello.connection_epoch){throw Error('Wrong guest state identity');}
            ['dataset_revision','state_rev','render_rev','render_key','worker_epoch'].forEach(function(k){P.counter(s[k],k==='worker_epoch');});P.bbox(s.bbox_dbu);P.pixels(s.pixels[0],s.pixels[1]);
            if(!(Number(P.decimal(s.dbu_um))>0)){throw Error('Invalid shared coordinate unit');}
            if(typeof s.rendering!=='boolean'){throw Error('Invalid guest render status');}
            if(state&&P.compare(s.state_rev,state.state_rev)<0){throw Error('Stale guest state');}
            if(session.mode==='explore'&&(!['low','medium','high','exact'].includes(s.detail)||!['auto','keep','cull'].includes(s.thin))){throw Error('Invalid display policy');}
        }
        function sync(){if(session.mode!=='explore'){return;}['depth','detail','thin'].forEach(function(k){if(doc.activeElement!==el('guest-'+k)){el('guest-'+k).value=state[k];}});
            ['frames','labels','mono'].forEach(function(k){el('guest-'+k).checked=state[k];});}
        function flush(){if(!hello||!state||flight||accepted||!queue.length||flushTimer!==null){return;}
            // Same edit cadence as the owner; reserve room for frame ACKs and
            // heartbeats beneath the server's 60 control messages/sec limit.
            const remaining=65-(now()-lastSend);if(remaining>0){flushTimer=win.setTimeout(function(){flushTimer=null;flush();},remaining);return;}lastSend=now();
            const q=queue.shift();flight=send({type:'explore.set',view_id:hello.view_id,connection_epoch:hello.connection_epoch,base_state_rev:state.state_rev,body:q.body});if(q.notice){q.notice.sent(flight);}}
        function edit(body,notice){if(stopped||suspended||!hello||!state||session.mode!=='explore'){if(notice){notice.cancel();}return;}
            if(gesture&&gesture.active()){gesture.cancel();}
            if(queue.length>=16){if(notice){notice.cancel();}status('Input queue full; wait for this view.');return;}queue.push({body:body,notice:notice||null});if(focusReceipt){focusReceipt.changed();}flush();panelsChanged();}
        function settle(){if(accepted&&state&&P.compare(state.state_rev,accepted)>=0){accepted=null;}if(focusReceipt){focusReceipt.changed();}flush();}
        function resized(){if(queryTools){queryTools.changed();}paintOverlays(true);if(resize!==null){win.clearTimeout(resize);}resize=win.setTimeout(function(){resize=null;if(!state||!session||session.mode!=='explore'){return;}
                try{const r=viewport.getBoundingClientRect(),d=win.devicePixelRatio||1,w=Math.max(1,Math.floor(r.width*d)),h=Math.max(1,Math.floor(r.height*d));P.pixels(w,h);
                    if(w!==state.pixels[0]||h!==state.pixels[1]){edit({pixels:[w,h]});}}
                catch(e){status(e.message);}
            },80);}
        function incoming(event,token){if(token!==serial||stopped||suspended){return;}
            // A valid 4096-pair layer selection can exceed 64KiB. Match the
            // owner control-reply bound; frame headers remain separately capped.
            try{if(typeof event.data!=='string'){frame(event.data,token);return;}if(event.data.length>256*1024){throw Error('Guest control reply limit');}const v=JSON.parse(event.data);
                if(v.type==='share.hello'){if(hello||v.protocol!==1||v.bundle!==bundle||v.share_id!==id||v.mode!==session.mode||v.read_only!==true||
                    typeof v.query!=='boolean'||v.measure!==(v.mode==='explore')||v.mode==='follow'&&v.query||!['view_id','connection_epoch'].every(function(k){return /^[0-9a-f]{64}$/.test(v[k]);})){throw Error('Invalid guest handshake');}
                    hello=v;delay=500;status(v.mode==='follow'?'Following owner · read-only':'Independent view · read-only');controls();return;}
                if(!hello){throw Error('Guest handshake missing');}
                if(v.type==='share.state'){validateState(v);if(gesture&&gesture.active()&&state&&state.state_rev!==v.state_rev){gesture.cancel();}state=v;sync();compose();settle();controls();resized();panelsChanged();status((session.mode==='follow'?'Following owner':'Independent view')+' · read-only'+(v.rendering?' · rendering…':''));if(v.failure){status('Render failed: '+String(v.failure));}return;}
                if(v.type==='accepted'){if(v.seq!==flight||v.view_id!==hello.view_id||v.connection_epoch!==hello.connection_epoch){throw Error('Wrong edit acknowledgment');}P.counter(v.state_rev);flight=null;accepted=v.state_rev;if(focusReceipt){focusReceipt.accepted(v.seq,v.state_rev);}settle();panelsChanged();return;}
                if(queryTools&&queryTools.receive(v)){return;}
                if(v.type==='error'){if(v.seq===flight){flight=accepted=null;queue=[];if(focusReceipt){focusReceipt.rejected(v.seq);}panelsChanged();status('View change rejected. No change was replayed.');}else{throw Error('Unexpected guest error');}return;}
                if(v.type!=='pong'){throw Error('Unsupported guest message');}
            }catch(_){stop('Guest protocol error. Reload with a valid invitation.',true);}
        }
        async function join(){if(joining||socket||stopped||suspended||!auth){return;}joining=true;controls();const token=serial;
            try{const s=await request('GET','/session');if(token!==serial||stopped||suspended){return;}
                if(!s||s.share_id!==id||s.read_only!==true||!['follow','explore'].includes(s.mode)||s.delivery!==(s.mode==='follow'?'follow_frames':'explore_frames')){stop('Invalid shared session. Ask for a new invitation.',true);return;}
                session=s;el('guest-mode').textContent=s.mode==='follow'?'Follow · read-only':'Explore · read-only';
                if(s.drc&&(!/^[0-9a-f]{64}$/.test(s.drc.id)||typeof s.drc.revision!=='string'||!s.drc.revision||s.drc.revision.length>128)){throw Error('Invalid DRC grant');}
                el('guest-scope').textContent='Approved layout layers and loaded levels only.'+(s.drc?' The entire named DRC result was separately approved; review notes and writes are excluded.':' DRC results are not shared.');
                const ws=new o.WebSocket(o.location.origin.replace(/^http:/,'ws:')+base+'/events',['floe.v1','bundle.'+bundle,'guest-csrf.'+auth.csrf]);
                socket=ws;seq='0';ws.binaryType='arraybuffer';ws.onmessage=function(e){incoming(e,token);};ws.onerror=function(){if(token===serial&&!stopped&&!suspended){status('Guest connection unavailable.');}};
                ws.onclose=function(){if(token!==serial||stopped||suspended){return;}disconnect();status('Disconnected; checking this share before reconnecting…');retry();};
                ws.onopen=function(){if(token!==serial){ws.close();return;}ping=win.setInterval(function(){if(hello){send({type:'ping'});}},10000);};
            }catch(e){if(token!==serial||stopped||suspended){return;}if([401,403,404,409].includes(e.status)){stop('Share expired, revoked, or changed. Ask the owner for a new invitation.',true);}else{status(e.message);retry();}}
            finally{if(token===serial){joining=false;controls();}}
        }
        function retry(){if(stopped||suspended||reconnect!==null){return;}reconnect=win.setTimeout(function(){reconnect=null;join();},delay);delay=Math.min(delay*2,5000);}
        async function start(){if(started){return;}started=true;const fragment=o.location.hash;
            try{if(fragment){o.history.replaceState(null,'',o.location.pathname);}if(!id||!ctx){throw Error('Invalid guest page');}
                if(typeof o.XHR!=='function'||typeof o.WebSocket!=='function'||typeof win.requestAnimationFrame!=='function'||typeof win.cancelAnimationFrame!=='function'){throw Error('Canvas, WebSocket and animation APIs are required.');}
                if(/^#invite=[0-9a-f]{64}$/.test(fragment)){remove();const a=await request('POST','/exchange',{invite:fragment.slice(8),protocol:1,bundle:bundle});if(stopped){return;}if(!valid(a)){throw Error('Invalid guest credentials');}auth=a;
                    try{win.sessionStorage.setItem(key,JSON.stringify(auth));}catch(_){status('Tab storage unavailable; reloading needs a new invitation.');}}
                else{if(fragment){throw Error('Invalid invitation; owner links are not accepted here');}try{const text=win.sessionStorage.getItem(key);auth=text&&text.length<=4096?JSON.parse(text):null;}catch(_){auth=null;}if(!valid(auth)){throw Error('Open a new guest invitation from the owner.');}}
                suspended=pageSuspended||!!doc.hidden;controls();if(win.ResizeObserver){observer=new win.ResizeObserver(resized);if(!suspended){observer.observe(viewport);}}await join();
            }catch(e){if(!stopped){stop(e.message||'Guest connection failed. No invitation was retried.',true);}}
        }
        el('guest-reconnect').onclick=function(){if(reconnect!==null){win.clearTimeout(reconnect);reconnect=null;}suspended=pageSuspended||!!doc.hidden;join();};
        el('guest-leave').onclick=async function(){if(!auth||stopped){return;}const proof=auth;stop('Leaving share…',false);remove();
            try{await request('DELETE','/session');status('Share left. The owner and other guests remain connected.');}
            catch(_){status('Local display cleared; server logout unconfirmed. Ask the owner to revoke this share. No retry was sent.');}
            finally{if(auth===proof){auth=null;}controls();}};
        function nav(n){if(n.kind==='zoom'&&!n.anchor){n.anchor=[0.5,0.5];}edit({navigation:n});}
        el('guest-fit').onclick=function(){nav({kind:'fit'});};el('guest-in').onclick=function(){nav({kind:'zoom',factor:0.8});};el('guest-out').onclick=function(){nav({kind:'zoom',factor:1.25});};
        ['depth','detail','thin'].forEach(function(k){el('guest-'+k).onchange=function(){const b={};b[k]=el('guest-'+k).value;edit(b);};});
        ['frames','labels','mono'].forEach(function(k){el('guest-'+k).onchange=function(){const b={};b[k]=el('guest-'+k).checked;edit(b);};});
        el('guest-go').onclick=function(){try{const n={kind:'goto',center_um:[P.decimal(el('guest-x').value.trim()),P.decimal(el('guest-y').value.trim())]},w=el('guest-width').value.trim();if(w){n.width_um=P.decimal(w);}nav(n);}catch(e){status(e.message);}};
        viewport.addEventListener('keydown',function(e){if(e.isComposing||e.ctrlKey||e.metaKey||e.altKey||!session){return;}
            if(e.key==='Escape'&&gesture&&gesture.active()){e.preventDefault();gesture.cancel();return;}
            if(drc&&(!queryTools||!queryTools.active())&&drc.key(e.key,e.shiftKey)){e.preventDefault();cursor();return;}
            if(session.mode!=='explore'){return;}
            if(queryTools&&queryTools.key(e.key)){e.preventDefault();return;}
            const a=e.shiftKey?0.1:0.5,pan={ArrowLeft:[-a,0],ArrowRight:[a,0],ArrowUp:[0,a],ArrowDown:[0,-a]};let n;
            if(pan[e.key]){n={kind:'pan',x:pan[e.key][0],y:pan[e.key][1],snap:true};}else if(e.key==='+'||e.key==='='){n={kind:'zoom',factor:0.8};}else if(e.key==='-'){n={kind:'zoom',factor:1.25};}else if(e.key==='Home'){n={kind:'fit'};}
            if(n){e.preventDefault();nav(n);}});
        viewport.addEventListener('mousedown',function(){viewport.focus();});
        // Follow can select its own DRC records but must never acquire the
        // camera gesture controller (even temporarily for marker clicking).
        let followDown=null;
        viewport.addEventListener('mousedown',function(e){followDown=session&&session.mode==='follow'&&e.button===0&&!e.altKey?{x:e.clientX,y:e.clientY,rev:state&&state.state_rev,serial:serial}:null;});
        viewport.addEventListener('mouseup',function(e){const d=followDown;followDown=null;if(d&&drc&&session.mode==='follow'&&state&&d.rev===state.state_rev&&e.button===0&&!e.altKey&&
            d.serial===serial&&Math.hypot(e.clientX-d.x,e.clientY-d.y)<=3){drc.click(e.clientX,e.clientY,e.detail===2,e);}});
        if(o.gestures){
            function pointerReady(){const c=queryContext(true);return !decode&&!!c&&!!o.query.scope(c,P,true);}
            gesture=o.gestures.bind({viewport:viewport,window:win,document:doc,ready:pointerReady,
                dimensions:function(){return queryContext(true).size;},stamp:function(){const r=canvas.getBoundingClientRect();return hello&&state?[hello.view_id,hello.connection_epoch,state.state_rev,r.left,r.top,r.width,r.height].join(':'):'';},
                requestAnimationFrame:win.requestAnimationFrame.bind(win),cancelAnimationFrame:win.cancelAnimationFrame.bind(win),
                preview:function(p,paint){dragShift=p;if(p&&queryTools){queryTools.changed();}if(paint){compose();}},cursor:cursor,pan:nav,band:nav,bandReady:pointerReady,notice:status,objectClicks:true,
                bandPreview:function(b){const box=el('zoom-band'),hint=el('zoom-band-hint');box.hidden=hint.hidden=!b;if(!b){return;}
                    const d=b.dimensions,x=b.start[0]*d.pixels[0],y=b.start[1]*d.pixels[1],ex=b.end[0]*d.pixels[0],ey=b.end[1]*d.pixels[1];
                    box.style.left=(d.left+Math.round(Math.min(x,ex))/d.dpr)+'px';box.style.top=(d.top+Math.round(Math.min(y,ey))/d.dpr)+'px';
                    box.style.width=(Math.max(1,Math.round(Math.abs(ex-x)))/d.dpr)+'px';box.style.height=(Math.max(1,Math.round(Math.abs(ey-y)))/d.dpr)+'px';
                    box.style.borderWidth=(1/d.dpr)+'px';hint.textContent=(b.outward?'Zoom out':'Zoom in')+' · release to apply · Esc cancels';},
                selectionMode:function(){return !!drc&&drc.active();},
                click:function(x,y,twice,m){if(queryTools&&queryTools.active()){queryTools.click(x,y,m);return;}if(drc&&drc.click(x,y,twice,m)){return;}if(queryTools){queryTools.click(x,y,m);}}});
            viewport.addEventListener('mousemove',function(e){if(!gesture.active()){if(drc){drc.move(e.clientX,e.clientY);}if(queryTools&&(!drc||!drc.active())){queryTools.move(e.clientX,e.clientY,e);}}});
            viewport.addEventListener('mouseleave',function(){followDown=null;if(drc){drc.move(NaN,NaN);}if(queryTools){queryTools.move(NaN,NaN);}});
            viewport.addEventListener('wheel',function(e){if(!session||session.mode!=='explore'){return;}e.preventDefault();
                const c=queryContext(false);if(!c||!pointerReady()||gesture.active()||state.rendering||!c.frame.final){return;}
                const n=o.gestures.wheelNavigation(e,c.size,c.rect);if(n){nav(n);}}, {passive:false});
        }
        win.addEventListener('resize',resized);
        doc.addEventListener('visibilitychange',function(){if(stopped){return;}suspended=pageSuspended||!!doc.hidden;if(suspended){disconnect();abortReads();if(observer){observer.disconnect();}status('Guest paused while hidden.');}else{if(observer){observer.observe(viewport);}join();}});
        win.addEventListener('pagehide',function(){pageSuspended=true;if(stopped){return;}suspended=true;disconnect();abortReads();if(observer){observer.disconnect();}});
        win.addEventListener('pageshow',function(e){if(e.persisted&&!stopped){pageSuspended=false;suspended=!!doc.hidden;if(observer&&!suspended){observer.observe(viewport);}status(suspended?'Guest paused while hidden.':'Restored; reconnecting without replaying commands.');join();}});
        return {start:start,stop:function(){stop('Guest stopped.',false);}};
    }
    const api={bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuest=api;api.bind({window:root,document:document,location:location,history:history,XHR:root.XMLHttpRequest,WebSocket:root.WebSocket,protocol:root.FloeProtocol,
        drc:root.FloeGuestDRC,drcSteps:root.FloeGuestDRCStep,focusReceipt:root.FloeGuestFocusReceipt,geometry:root.FloeDRCGeometry,selection:root.FloeDRCGroups,
        layers:root.FloeGuestLayers,
        display:root.FloeGuestDisplay,tools:root.FloeGuestTools,queryWire:root.FloeGuestQueryWire,query:root.FloeQuery,
        inspect:root.FloeInspect,measure:root.FloeMeasure,rulers:root.FloeRulers,gestures:root.FloeGestures,
        decode:function(h,data,cb){return root.FloeImageDecode.create({Image:root.Image,ImageData:root.ImageData,Blob:root.Blob,URL:root.URL,setTimeout:root.setTimeout.bind(root),clearTimeout:root.clearTimeout.bind(root)},h,data,cb);}}).start();}
}(typeof window==='object'?window:this));
