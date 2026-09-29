/* ES2017. Session-only client. Never imports owner auth, file or write modules. */
(function(root) {
    'use strict';
    function bind(o) {
        const win=o.window,doc=o.document,P=o.protocol,el=function(id){return doc.getElementById('server-'+id);};
        const match=/^\/server\/([0-9a-f]{64})$/.exec(o.location.pathname),id=match?match[1]:'';
        const bundle=doc.querySelector('meta[name="floe-bundle"]').content;
        const transport=doc.querySelector('meta[name="floe-http-test"]'),httpTest=!!transport&&transport.content==='true';
        const base='/api/v1/server/sessions/'+id,key='floe-server-session:'+o.location.origin+':'+id;
        const canvas=el('canvas'),port=el('viewport'),ctx=canvas.getContext('2d',{alpha:false});
        const inputs=['fit','in','out','depth','detail','thin','frames','labels','mono','x','y','width','go'];
        let auth=null,socket=null,hello=null,state=null,seq='0',serial=0,started=false,stopped=false,hidden=false,joining=false;
        let fresh=false,unopened=false,publicDemo=false,flight=null,accepted=null,queue=[],decode=null,raf=null,image=null,shift=null,gesture=null,releasedPan=false;
        let retryTimer=null,flushTimer=null,resizeTimer=null,connectTimer=null,editTimer=null,ping=null,observer=null,delay=500,lastSend=-Infinity,received=0;
        const requests=new Set(),now=o.now||function(){return win.performance.now();};
        function status(text){el('status').textContent=text;}
        function valid(a){return !!a&&a.launch_id===id&&/^[0-9a-f]{64}$/.test(a.csrf)&&a.bundle===bundle&&a.protocol==='floe-server-v1';}
        function remove(){try{win.sessionStorage.removeItem(key);}catch(_) {}}
        function ready(){return !stopped&&!hidden&&!!hello&&!!state&&!state.failure&&!['failed','closed'].includes(state.status)&&!!socket&&socket.readyState===1;}
        function controls(){inputs.forEach(function(n){el(n).disabled=!ready()||(n==='labels'&&state.capabilities.labels!==true);});
            el('reconnect').disabled=stopped||!auth||joining||hidden;el('leave').disabled=stopped||!auth;
            el('open').hidden=!unopened;el('open').disabled=stopped||hidden||joining||!auth;}
        function clear(){image=null;shift=null;releasedPan=false;canvas.width=canvas.height=1;el('empty').hidden=false;el('frame-status').textContent='No displayed frame';}
        function timerClear(name){if(name!==null){win.clearTimeout(name);}return null;}
        function disconnect(){serial++;joining=false;const old=socket;socket=null;hello=state=null;flight=accepted=null;queue=[];
            requests.forEach(function(x){if(!x.exchange){x.abort();}});
            retryTimer=timerClear(retryTimer);flushTimer=timerClear(flushTimer);resizeTimer=timerClear(resizeTimer);
            connectTimer=timerClear(connectTimer);editTimer=timerClear(editTimer);
            if(ping!==null){win.clearInterval(ping);ping=null;}if(gesture){gesture.cancel();}shift=null;
            if(decode){const job=decode;decode=null;job.cancel();}if(raf!==null){win.cancelAnimationFrame(raf);raf=null;}
            if(old){old.onmessage=old.onclose=old.onerror=old.onopen=null;old.close();}clear();controls();}
        function stop(text,forget){stopped=true;disconnect();if(observer){observer.disconnect();}if(forget){remove();auth=null;}status(text);el('samples').hidden=!publicDemo;controls();}
        function request(method,suffix,body){
            const allowed={'/exchange':['POST'],'':['GET','DELETE'],'/view':['GET','POST']};
            if(!allowed[suffix]||!allowed[suffix].includes(method)){return Promise.reject(Error('Unsupported server request'));}
            return new Promise(function(resolve,reject){const x=new o.XHR();x.exchange=suffix==='/exchange';requests.add(x);let done=false;
                function end(error,value){if(done){return;}done=true;requests.delete(x);if(error){reject(error);}else{resolve(value);}}
                x.open(method,base+suffix,true);x.timeout=8000;
                if(auth){x.setRequestHeader('X-Floe-CSRF',auth.csrf);}if(body!==undefined){x.setRequestHeader('Content-Type','application/json');}
                x.onprogress=function(e){if(e.loaded>256*1024||e.lengthComputable&&e.total>256*1024){end(Error('Server reply limit'));x.abort();}};
                x.onload=function(){if(x.status<200||x.status>=300){const e=Error('Server request unavailable (HTTP '+x.status+').');e.status=x.status;end(e);return;}
                    try{if(x.responseText.length>256*1024){throw Error('Reply limit');}end(null,x.responseText?JSON.parse(x.responseText):null);}catch(_){end(Error('Invalid server reply'));}};
                x.onerror=x.ontimeout=function(){end(Error('Connection unavailable. No command was replayed.'));};
                x.onabort=function(){end(Error('Request cancelled'));};x.send(body===undefined?null:JSON.stringify(body));
            });
        }
        function dimensions(){const r=port.getBoundingClientRect(),d=win.devicePixelRatio||1;
            let w=Math.max(1,Math.floor(r.width*d)),h=Math.max(1,Math.floor(r.height*d));
            if(publicDemo){const s=Math.min(1,2048/w,2048/h,Math.sqrt(2097152/(w*h)));w=Math.max(1,Math.floor(w*s));h=Math.max(1,Math.floor(h*s));}
            const p=[w,h];P.pixels(p[0],p[1]);return p;}
        function send(v){if(!socket||socket.readyState!==1||!hello){throw Error('Disconnected');}
            const next=P.next(seq),text=JSON.stringify(Object.assign({},v,{seq:next}));
            if(socket.bufferedAmount>16384||new TextEncoder().encode(text).length>8192){throw Error('Input limit');}
            seq=next;socket.send(text);return seq;}
        function ack(h){send({type:'frame.ack',connection_epoch:hello.connection_epoch,frame_id:h.frame_id});}
        function compose(){if(!state){return;}
            const same=image&&['view_id','connection_epoch','dataset_revision','worker_epoch','render_key'].every(function(k){return image.header[k]===state[k];});
            if(!same||state.status==='closed'){clear();return;}
            const exact=P.matches(image.header,state),p=exact?[0,0]:P.placement(image.header,state);
            // placement is an exact raster-phase contract, not a condition for
            // retaining pixels already displayed in this session. Zoom and a
            // non-16px pan freeze the last composite until its replacement;
            // they must never turn it into a current receipt or query target.
            if(p&&!releasedPan){
                canvas.width=state.pixels[0];canvas.height=state.pixels[1];ctx.imageSmoothingEnabled=false;
                ctx.fillStyle='#000';ctx.fillRect(0,0,canvas.width,canvas.height);
                ctx.drawImage(image.canvas,-p[0]-(shift||[0,0])[0],-p[1]-(shift||[0,0])[1]);
            }
            el('empty').hidden=true;
            if(state.failure){el('frame-status').textContent='Last image · renderer failed';}
            else if(!exact||releasedPan){el('frame-status').textContent='Previous image · waiting for current frame';}
            else {const h=image.header;el('frame-status').textContent=(h.complete?'Complete':'Partial / incomplete')+(h.approximate?' · approximate':'')+' · '+h.width+' × '+h.height+' px';}}
        function frame(data,token){const packet=P.packet(data),h=packet.header;
            if(!hello||h.connection_epoch!==hello.connection_epoch||h.view_id!==id||h.purpose!=='foreground'||h.query){throw Error('Wrong server frame identity');}
            if(decode||raf!==null){throw Error('Frame credit exceeded');}if(!ready()||!P.matches(h,state)){ack(h);return;}
            const next=doc.createElement('canvas');next.width=h.width;next.height=h.height;
            const job=o.decode(h,packet.data,function(draw,error){if(token!==serial||stopped||hidden){return;}decode=null;
                if(error){lost('Frame decode failed; reconnecting.');return;}
                if(!draw||!ready()||!P.matches(h,state)){ack(h);return;}
                try{draw(next.getContext('2d',{alpha:false}));}catch(_){lost('Canvas failed; reconnecting.');return;}
                raf=win.requestAnimationFrame(function(){raf=null;if(token!==serial||stopped||hidden){return;}
                    try{if(ready()&&P.matches(h,state)){image={header:h,canvas:next};compose();}ack(h);}
                    catch(_){lost('Presentation failed; reconnecting.');}
                });
            });decode=job;job.start();
        }
        function validate(s){if(!hello||s.view_id!==id||s.connection_epoch!==hello.connection_epoch){throw Error('Wrong server state identity');}
            ['state_rev','render_rev','render_key','dataset_revision','worker_epoch'].forEach(function(k){P.counter(s[k],k==='worker_epoch');});
            P.bbox(s.bbox_dbu);if(!Array.isArray(s.pixels)){throw Error('Invalid pixels');}P.pixels(s.pixels[0],s.pixels[1]);
            if(!s.capabilities||s.capabilities.query!==false||s.capabilities.clip!==false||s.capabilities.mode!==false||
                !['low','medium','high','exact'].includes(s.detail)||!['auto','keep','cull'].includes(s.thin)||
                !['opening','rendering','cancelling','idle','failed','closed'].includes(s.status)){throw Error('Invalid server state');}
            if(state&&P.compare(s.state_rev,state.state_rev)<0){throw Error('Regressed server state');}
        }
        function sync(){['depth','detail','thin'].forEach(function(k){if(doc.activeElement!==el(k)){el(k).value=state[k];}});
            ['frames','labels','mono'].forEach(function(k){el(k).checked=state[k];});}
        function settle(){if(accepted&&state&&P.compare(state.state_rev,accepted)>=0){accepted=null;if(releasedPan){releasedPan=false;shift=null;compose();}}if(!flight&&!accepted){editTimer=timerClear(editTimer);}flush();}
        function flush(){if(!ready()||flight||accepted||!queue.length){return;}const wait=100-(now()-lastSend);
            if(wait>0){if(flushTimer===null){flushTimer=win.setTimeout(function(){flushTimer=null;flush();},wait);}return;}
            const body=queue.shift();try{flight=send({type:'view.set',connection_epoch:hello.connection_epoch,view_id:id,base_state_rev:state.state_rev,body:body});lastSend=now();
                editTimer=win.setTimeout(function(){editTimer=null;lost('Edit outcome unknown; checking current view without replay.');},10000);}
            catch(_){lost('Input not confirmed; reconnecting without replay.');}}
        function edit(body){if(!ready()){return;}if(queue.length>=8){status('Input queue full; wait for this view.');return;}queue.push(body);flush();}
        function incoming(event,token){if(token!==serial||stopped||hidden){return;}
            received=now();
            try{if(typeof event.data!=='string'){frame(event.data,token);return;}if(event.data.length>256*1024){throw Error('Control limit');}const v=JSON.parse(event.data);
                if(v.type==='hello'){if(hello||v.protocol!==1||v.bundle!==bundle||v.view_id!==id||!/^[0-9a-f]{64}$/.test(v.connection_epoch)||
                    v.frame_credit!==1||!v.capabilities||v.capabilities.view!==true||['index','review','export','query'].some(function(k){return v.capabilities[k]!==false;})){throw Error('Invalid handshake');}
                    hello=v;connectTimer=timerClear(connectTimer);delay=500;controls();return;}
                if(!hello){throw Error('Missing handshake');}
                if(v.type==='opening'){if(v.view_id!==id||v.connection_epoch!==hello.connection_epoch){throw Error('Wrong opening identity');}status('Opening assigned layout…');return;}
                if(v.type==='snapshot'){validate(v);if(state&&state.state_rev!==v.state_rev){if(gesture){gesture.cancel();}releasedPan=false;shift=null;}state=v;
                    if(v.failure){queue=[];}sync();compose();controls();settle();resized();status(v.failure?'Renderer failed; end this session and request a new launch.':'Connected · '+v.status);return;}
                if(v.type==='accepted'){if(!flight||v.seq!==flight){throw Error('Wrong acknowledgment');}P.counter(v.state_rev);flight=null;accepted=v.state_rev;settle();return;}
                if(v.type==='error'){if(!flight||v.seq!==flight){throw Error('Wrong error response');}flight=null;queue=[];accepted=null;releasedPan=false;shift=null;compose();editTimer=timerClear(editTimer);status('View edit rejected; check current state before trying again.');return;}
                if(v.type==='pong'){return;}throw Error('Unknown server message');
            }catch(_){stop('Invalid server protocol; reload with a compatible client.',false);}
        }
        function retry(){if(stopped||hidden||retryTimer!==null){return;}retryTimer=win.setTimeout(function(){retryTimer=null;join();},delay);delay=Math.min(5000,delay*2);}
        function lost(text){disconnect();status(text);retry();}
        async function join(){if(stopped||hidden||joining||!auth){return;}disconnect();joining=true;unopened=false;const token=serial;controls();
            try{const session=await request('GET','');if(token!==serial||stopped||hidden){return;}
                if(session.launch_id!==id||session.viewer_ready!==true||session.render_transport!==true||!session.principal||
                    typeof session.principal.namespace!=='string'||typeof session.principal.subject!=='string'){stop('Server viewer unavailable.',false);return;}
                publicDemo=session.public_demo===true;
                el('user').textContent=publicDemo?'Public demo · independent read-only view · session up to 15 minutes':session.principal.namespace+' / '+session.principal.subject;
                let view=await request('GET','/view');if(token!==serial||stopped||hidden){return;}
                if(view.view_id!==id){throw Error('Wrong view');}
                if(view.type==='unopened'){
                    if(!fresh){unopened=true;status('Layout is not open. Open assigned layout to try once.');return;}
                    fresh=false;const p=dimensions();await request('POST','/view',{width:p[0],height:p[1]});if(token!==serial||stopped||hidden){return;}
                }else if(!['opening','snapshot'].includes(view.type)){throw Error('Invalid view response');}
                fresh=false;
                const ws=new o.WebSocket(o.location.origin.replace(/^http/,'ws')+base+'/stream',['floe-server-v1','bundle.'+bundle,'csrf.'+auth.csrf]);
                socket=ws;seq='0';ws.binaryType='arraybuffer';ws.onmessage=function(e){incoming(e,token);};
                received=now();connectTimer=win.setTimeout(function(){connectTimer=null;if(token===serial){lost('Connection handshake timed out; checking session.');}},10000);
                ws.onerror=function(){if(token===serial){status('Server connection unavailable.');}};
                ws.onclose=function(){if(token===serial&&!stopped&&!hidden){lost('Disconnected; checking session before reconnecting…');}};
                ws.onopen=function(){if(token!==serial||stopped||hidden){ws.close();return;}ping=win.setInterval(function(){if(now()-received>30000){lost('Connection became unresponsive; checking session.');return;}if(hello){try{send({type:'ping'});}catch(_){lost('Connection unavailable.');}}},10000);};
            }catch(e){if(token!==serial||stopped||hidden){return;}if([401,403,404].includes(e.status)){stop('Session expired or revoked. Request a new launch.',true);}
                else{status(e.status===429?'Server capacity busy; no open request was repeated.':e.message);retry();}}
            finally{if(token===serial){joining=false;controls();}}
        }
        function resized(){resizeTimer=timerClear(resizeTimer);if(!ready()){return;}resizeTimer=win.setTimeout(function(){resizeTimer=null;if(!ready()){return;}
            try{const p=dimensions();if(p[0]===state.pixels[0]&&p[1]===state.pixels[1]){return;}edit({pixels:p});}catch(e){status(e.message);}},120);}
        async function start(){if(started){return;}started=true;const fragment=o.location.hash;
            try{if(fragment){o.history.replaceState(null,'',o.location.pathname);}if(!id||o.location.protocol!==(httpTest?'http:':'https:')||!ctx){throw Error('Open the configured '+(httpTest?'HTTP test':'HTTPS')+' server session link.');}
                doc.getElementById('transport-warning').hidden=!httpTest;
                if(/^#bootstrap=[0-9a-f]{64}$/.test(fragment)){
                    remove();const a=await request('POST','/exchange',{bootstrap:fragment.slice(11)});if(stopped){return;}
                    if(!valid(a)||!a.viewer_ready||!a.render_transport){throw Error('Server viewer unavailable.');}auth=a;fresh=true;
                    try{win.sessionStorage.setItem(key,JSON.stringify(auth));}catch(_){status('Tab storage unavailable; reloading will need a new launch.');}
                }else{if(fragment){throw Error('Invalid server launch link.');}try{const text=win.sessionStorage.getItem(key);auth=text&&text.length<=4096?JSON.parse(text):null;}catch(_){auth=null;}
                    if(!valid(auth)){throw Error('Open a new server launch link.');}}
                hidden=hidden||!!doc.hidden;if(win.ResizeObserver){observer=new win.ResizeObserver(resized);observer.observe(port);}await join();
            }catch(e){if(!stopped){stop(e.message||'Launch failed. The one-use link was not retried.',true);}}
        }
        el('reconnect').onclick=function(){join();};
        el('open').onclick=function(){if(unopened&&!joining&&!stopped&&!hidden){fresh=true;join();}};
        el('leave').onclick=async function(){if(!auth||stopped||!win.confirm('End only this server session? Shared files and other users remain unchanged.')){return;}
            stop('Ending this session…',false);remove();try{await request('DELETE','');status('Session ended. Shared files and other users are unchanged.');}
            catch(_){status('Display cleared; server logout unconfirmed. This session will expire automatically. No retry was sent.');}finally{auth=null;controls();}};
        function nav(n){if(n.kind==='zoom'&&!n.anchor){n.anchor=[0.5,0.5];}edit({navigation:n});}
        el('fit').onclick=function(){nav({kind:'fit'});};el('in').onclick=function(){nav({kind:'zoom',factor:0.8});};el('out').onclick=function(){nav({kind:'zoom',factor:1.25});};
        ['depth','detail','thin'].forEach(function(k){el(k).onchange=function(){const value=el(k).value;if(k==='depth'&&!/^(full|[0-9]{1,3})$/.test(value)){status('Depth must be full or an integer.');return;}const p={};p[k]=value;edit(p);};});
        ['frames','labels','mono'].forEach(function(k){el(k).onchange=function(){const p={};p[k]=el(k).checked;edit(p);};});
        el('go').onclick=function(){try{const n={kind:'goto',center_um:[P.decimal(el('x').value),P.decimal(el('y').value)]};if(el('width').value){n.width_um=P.decimal(el('width').value);if(Number(n.width_um)<=0){throw Error('Width must be positive');}}nav(n);}catch(e){status(e.message);}};
        port.addEventListener('keydown',function(e){if(e.isComposing||e.keyCode===229){return;}
            if(e.key==='Escape'&&gesture&&gesture.active()){e.preventDefault();gesture.cancel();return;}
            if(e.ctrlKey||e.metaKey||e.altKey||!ready()){return;}const step=e.shiftKey?0.1:0.5;
            const dirs={ArrowLeft:[-step,0],ArrowRight:[step,0],ArrowUp:[0,step],ArrowDown:[0,-step]};
            if(dirs[e.key]){e.preventDefault();nav({kind:'pan',x:dirs[e.key][0],y:dirs[e.key][1],snap:true});}
            else if(['+','=','-','Home'].includes(e.key)){e.preventDefault();nav(e.key==='Home'?{kind:'fit'}:{kind:'zoom',factor:e.key==='-'?1.25:0.8});}
        });
        function screen(){const r=port.getBoundingClientRect(),scale=Math.min(r.width/state.pixels[0],r.height/state.pixels[1]);
            return {pixels:state.pixels,dpr:1/scale,left:(r.width-state.pixels[0]*scale)/2,top:(r.height-state.pixels[1]*scale)/2};}
        if(o.gestures){gesture=o.gestures.bind({window:win,document:doc,viewport:port,ready:function(){return ready()&&!!image&&P.matches(image.header,state)&&!flight&&!accepted&&!queue.length;},
            stamp:function(){return state&&hello.connection_epoch+':'+state.state_rev;},dimensions:screen,
            requestAnimationFrame:win.requestAnimationFrame.bind(win),cancelAnimationFrame:win.cancelAnimationFrame.bind(win),
            preview:function(p,paint){shift=p;releasedPan=!paint;if(paint){compose();}},
            band:nav,bandReady:function(){return !!image&&P.matches(image.header,state);},notice:status,
            bandPreview:function(b){
                const box=el('zoom-band'),hint=el('zoom-band-hint');box.hidden=hint.hidden=!b;if(!b){return;}
                const d=b.dimensions,x=b.start[0]*d.pixels[0],y=b.start[1]*d.pixels[1],ex=b.end[0]*d.pixels[0],ey=b.end[1]*d.pixels[1];
                box.style.left=((d.left||0)+Math.round(Math.min(x,ex))/d.dpr)+'px';box.style.top=((d.top||0)+Math.round(Math.min(y,ey))/d.dpr)+'px';
                box.style.width=(Math.max(1,Math.round(Math.abs(ex-x)))/d.dpr)+'px';box.style.height=(Math.max(1,Math.round(Math.abs(ey-y)))/d.dpr)+'px';
                box.style.borderWidth=(1/d.dpr)+'px';hint.textContent=(b.outward?'Zoom out':'Zoom in')+' · release to apply · Esc cancels';
            },cursor:function(d){port.style.cursor=d?(gesture&&gesture.bandActive()?'crosshair':'grabbing'):'';},pan:nav});
            port.addEventListener('wheel',function(e){e.preventDefault();if(!ready()||flight||accepted||queue.length){return;}const n=o.gestures.wheelNavigation(e,screen(),port.getBoundingClientRect());if(n){nav(n);}},{passive:false});}
        function pause(){hidden=true;disconnect();status('Hidden · disconnected. Reconnecting does not replay input.');}
        function resume(){hidden=!!doc.hidden;if(!hidden&&!stopped){join();}}
        doc.addEventListener('visibilitychange',function(){if(doc.hidden){pause();}else{resume();}});
        win.addEventListener('pagehide',pause);win.addEventListener('pageshow',resume);win.addEventListener('resize',resized);
        controls();return {start:start,stop:function(){stop('Session display closed.',false);}};
    }
    if(typeof module==='object'&&module.exports){module.exports={bind:bind};}
    else{const w=root,c=bind({window:w,document:w.document,location:w.location,history:w.history,protocol:w.FloeProtocol,gestures:w.FloeGestures,
        XHR:w.XMLHttpRequest,WebSocket:w.WebSocket,decode:function(h,d,done){return w.FloeImageDecode.create(w,h,d,done);}});c.start();}
}(typeof window==='object'?window:this));
