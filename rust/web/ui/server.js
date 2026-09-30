/* ES2017. Session-only client. Never imports owner auth, file or write modules. */
(function(root) {
    'use strict';
    function bind(o) {
        const win=o.window,doc=o.document,P=o.protocol,V=o.viewer,el=function(id){return doc.getElementById('server-'+id);};
        const match=/^\/server\/([0-9a-f]{64})$/.exec(o.location.pathname),id=match?match[1]:'';
        const bundle=doc.querySelector('meta[name="floe-bundle"]').content;
        const transport=doc.querySelector('meta[name="floe-http-test"]'),httpTest=!!transport&&transport.content==='true';
        const base='/api/v1/server/sessions/'+id,key='floe-server-session:'+o.location.origin+':'+id;
        const canvas=el('canvas'),port=el('viewport'),ctx=canvas.getContext('2d',{alpha:false});
        const inputs=['fit','zoom-in','zoom-out','depth','detail','thin','frames','labels','mono','font-px','goto-x','goto-y','goto-width','goto'];
        let auth=null,socket=null,hello=null,state=null,seq='0',serial=0,started=false,stopped=false,hidden=false,joining=false;
        let fresh=false,unopened=false,publicDemo=false,flight=null,accepted=null,queue=[],decode=null,raf=null,image=null,shift=null,gesture=null,releasedPan=false;
        let lastPlacement=null,frozen=false,flightBody=null,palette=null,minimap=null,panes=null,menubar=null,cells=null,rowStyle=null;
        const callbacks=new WeakMap();
        let retryTimer=null,flushTimer=null,resizeTimer=null,connectTimer=null,editTimer=null,ping=null,observer=null,delay=500,lastSend=-Infinity,received=0;
        const requests=new Set(),now=o.now||function(){return win.performance.now();};
        function status(text){el('status').textContent=text;}
        function valid(a){return !!a&&a.launch_id===id&&/^[0-9a-f]{64}$/.test(a.csrf)&&a.bundle===bundle&&a.protocol==='floe-server-v1';}
        function remove(){try{win.sessionStorage.removeItem(key);}catch(_) {}}
        function ready(){return !stopped&&!hidden&&!!hello&&!!state&&!state.failure&&!['failed','closed'].includes(state.status)&&!!socket&&socket.readyState===1;}
        function viewerContext(){return {active:!stopped&&!hidden,connecting:joining||requests.size>0||!!socket&&(socket.readyState===0||!hello),
            connected:!!socket&&socket.readyState===1, state:state,frame:image&&image.header,
            pending:flight!==null||accepted!==null||queue.length>0,decoding:decode!==null,presenting:raf!==null,
            acked:!!image,gesture:!!(gesture&&gesture.active())};}
        function updateCursor(){
            V.cursor(el('shell'),port,V.busy(P,viewerContext()),gesture,false);
        }
        function controls(){inputs.forEach(function(n){el(n).disabled=!ready()||(['labels','font-px'].includes(n)&&state.capabilities.labels!==true);});
            el('reconnect').disabled=stopped||!auth||joining||hidden;el('leave').disabled=stopped||!auth;
            el('open').hidden=!unopened;el('open').disabled=stopped||hidden||joining||!auth;
            if(palette){palette.changed();}if(minimap){minimap.changed();}if(cells){cells.changed();}if(panes){panes.changed();}if(menubar){menubar.refresh();}updateCursor();}
        function clear(){image=null;shift=null;releasedPan=false;lastPlacement=null;frozen=false;canvas.width=canvas.height=1;el('empty').hidden=false;el('frame-status').textContent='No displayed frame';updateCursor();}
        function finishEdit(body,error){const done=body&&callbacks.get(body);if(body){callbacks.delete(body);}if(done){done(error);}}
        function rejectQueue(){queue.forEach(function(body){finishEdit(body,'Input was not applied.');});queue=[];}
        function timerClear(name){if(name!==null){win.clearTimeout(name);}return null;}
        function disconnect(){serial++;joining=false;const old=socket;socket=null;hello=state=null;flight=accepted=null;rejectQueue();finishEdit(flightBody,'Connection interrupted.');flightBody=null;
            requests.forEach(function(x){if(!x.exchange){x.abort();}});
            retryTimer=timerClear(retryTimer);flushTimer=timerClear(flushTimer);resizeTimer=timerClear(resizeTimer);
            connectTimer=timerClear(connectTimer);editTimer=timerClear(editTimer);
            if(ping!==null){win.clearInterval(ping);ping=null;}if(gesture){gesture.cancel();}shift=null;
            if(decode){const job=decode;decode=null;job.cancel();}if(raf!==null){win.cancelAnimationFrame(raf);raf=null;}
            if(old){old.onmessage=old.onclose=old.onerror=old.onopen=null;old.close();}clear();controls();}
        function stop(text,forget){stopped=true;disconnect();if(observer){observer.disconnect();}if(forget){remove();auth=null;}status(text);el('samples').hidden=!publicDemo;
            if(palette){palette.stop();}if(minimap){minimap.stop();}if(cells){cells.stop();}controls();}
        function request(method,suffix,body,token){
            const allowed=[['POST','/exchange'],['GET',''],['DELETE',''],['GET','/view'],['POST','/view'],['POST','/palette'],['GET','/presets'],['GET',/^\/minimap\/(full|[0-9]|[12][0-9]|3[01])$/],['GET',/^\/fill-slots\/[0-9a-f]{40}$/],['POST','/cells']];
            // A cell tree answer may list 20 000 children and waits for the worker's tree.
            const limit=suffix==='/cells'?8*1024*1024:256*1024;
            if(!allowed.some(function(a){return a[0]===method&&(typeof a[1]==='string'?a[1]===suffix:a[1].test(suffix));})){return Promise.reject(Error('Unsupported server request'));}
            return new Promise(function(resolve,reject){const x=new o.XHR();x.exchange=suffix==='/exchange';requests.add(x);updateCursor();let done=false;
                if(token){token.abort=function(){x.abort();};}
                function end(error,value){if(done){return;}done=true;requests.delete(x);updateCursor();if(error){reject(error);}else{resolve(value);}}
                x.open(method,base+suffix,true);x.timeout=suffix==='/cells'?20000:8000;
                if(auth){x.setRequestHeader('X-Floe-CSRF',auth.csrf);}if(body!==undefined){x.setRequestHeader('Content-Type','application/json');}
                x.onprogress=function(e){if(e.loaded>limit||e.lengthComputable&&e.total>limit){end(Error('Server reply limit'));x.abort();}};
                x.onload=function(){if(x.status<200||x.status>=300){const e=Error('Server request unavailable (HTTP '+x.status+').');e.status=x.status;end(e);return;}
                    try{if(x.responseText.length>limit){throw Error('Reply limit');}end(null,x.responseText?JSON.parse(x.responseText):null);}catch(_){end(Error('Invalid server reply'));}};
                x.onerror=x.ontimeout=function(){end(Error('Connection unavailable. No command was replayed.'));};
                x.onabort=function(){end(Error('Request cancelled'));};x.send(body===undefined?null:JSON.stringify(body));
            });
        }
        function http(method,path,body,_json,token){
            // The shared panels address owner routes; map exactly this
            // session's read-only equivalents and refuse everything else.
            let m,suffix=null;
            if(method==='POST'&&(m=/^\/api\/v1\/views\/([0-9a-f]{64})\/palette$/.exec(path))&&m[1]===id){suffix='/palette';}
            else if(method==='GET'&&(m=/^\/api\/v1\/views\/([0-9a-f]{64})\/minimap\/([a-z0-9]{1,4})$/.exec(path))&&m[1]===id){suffix='/minimap/'+m[2];}
            else if(method==='GET'&&(m=/^\/api\/v1\/views\/([0-9a-f]{64})\/fill-slots\/([0-9a-f]{40})$/.exec(path))&&m[1]===id){suffix='/fill-slots/'+m[2];}
            else if(method==='GET'&&path==='/api/v1/palette/presets'){suffix='/presets';}
            else if(method==='POST'&&(m=/^\/api\/v1\/views\/([0-9a-f]{64})\/cells$/.exec(path))&&m[1]===id){suffix='/cells';}
            if(suffix===null||!ready()){return Promise.reject(Error('Unavailable in this session'));}
            return request(method,suffix,body,token);
        }
        function dimensions(){return V.dimensions(P,port.getBoundingClientRect(),win.devicePixelRatio||1,
            publicDemo?{axis:2048,area:2097152}:null).pixels;}
        function send(v){if(!socket||socket.readyState!==1||!hello){throw Error('Disconnected');}
            const next=P.next(seq),text=JSON.stringify(Object.assign({},v,{seq:next}));
            if(socket.bufferedAmount>16384||new TextEncoder().encode(text).length>8192){throw Error('Input limit');}
            seq=next;socket.send(text);return seq;}
        function ack(h){send({type:'frame.ack',connection_epoch:hello.connection_epoch,frame_id:h.frame_id});}
        function compose(){if(!state){return;}
            const same=V.sameSource(image&&image.header,state);
            if(!same||state.status==='closed'){clear();return;}
            const exact=P.matches(image.header,state);
            lastPlacement=V.compose({protocol:P,canvas:canvas,foreground:frozen?null:image.header,state:state,size:screen(),delta:releasedPan?null:(shift||[0,0])});
            el('empty').hidden=true;
            if(state.failure){el('frame-status').textContent='Last image · renderer failed';}
            else if(!exact||releasedPan||flight||accepted||queue.length){el('frame-status').textContent='Previous image · waiting for current frame';}
            else {el('frame-status').textContent=V.frameStatus(image.header);}updateCursor();}
        function freezeDisplay(){if(image&&lastPlacement){frozen=V.freeze({canvas:canvas,placement:lastPlacement,document:doc,size:screen()})||frozen;
            lastPlacement={pixels:state.pixels,margin:null,full:false,foreground:[0,0]};}}
        function frame(data,token){const packet=P.packet(data),h=packet.header;
            if(!hello||h.connection_epoch!==hello.connection_epoch||h.view_id!==id||h.purpose!=='foreground'||h.query){throw Error('Wrong server frame identity');}
            if(decode||raf!==null){throw Error('Frame credit exceeded');}if(!ready()||!P.matches(h,state)){ack(h);return;}
            const next=doc.createElement('canvas');next.width=h.width;next.height=h.height;
            const job=o.decode(h,packet.data,function(draw,error){if(token!==serial||stopped||hidden){return;}decode=null;
                if(error){lost('Frame decode failed; reconnecting.');return;}
                if(!draw||!ready()||!P.matches(h,state)){ack(h);updateCursor();return;}
                try{V.paint(next,h,draw);}catch(_){lost('Canvas failed; reconnecting.');return;}
                raf=win.requestAnimationFrame(function(){raf=null;if(token!==serial||stopped||hidden){return;}
                    try{if(ready()&&P.matches(h,state)){V.paint(canvas,h,function(context){context.drawImage(next,0,0);});image={header:h,canvas:next};frozen=false;compose();}ack(h);updateCursor();}
                    catch(_){lost('Presentation failed; reconnecting.');}
                });updateCursor();
            });decode=job;updateCursor();job.start();
        }
        function validate(s){if(!hello||s.view_id!==id||s.connection_epoch!==hello.connection_epoch){throw Error('Wrong server state identity');}
            ['state_rev','render_rev','render_key','dataset_revision','worker_epoch'].forEach(function(k){P.counter(s[k],k==='worker_epoch');});
            P.bbox(s.bbox_dbu);if(!Array.isArray(s.pixels)){throw Error('Invalid pixels');}P.pixels(s.pixels[0],s.pixels[1]);
            if(!s.capabilities||s.capabilities.query!==false||s.capabilities.clip!==false||s.capabilities.mode!==false||
                !['low','medium','high','exact'].includes(s.detail)||!['auto','keep','cull'].includes(s.thin)||
                !['opening','rendering','cancelling','idle','failed','closed'].includes(s.status)){throw Error('Invalid server state');}
            if(state&&P.compare(s.state_rev,state.state_rev)<0){throw Error('Regressed server state');}
        }
        function restorePixels(){if(image&&state&&P.matches(image.header,state)){V.paint(canvas,image.header,function(context){context.drawImage(image.canvas,0,0);});frozen=false;}}
        function settle(){if(accepted&&state&&P.compare(state.state_rev,accepted)>=0){
            finishEdit(flightBody,state.state_rev===accepted?null:'The view changed again.');flightBody=null;
            accepted=null;releasedPan=false;shift=null;restorePixels();compose();}
            if(!flight&&!accepted){editTimer=timerClear(editTimer);}flush();updateCursor();}
        function flush(){if(!ready()||flight||accepted||!queue.length){return;}const wait=100-(now()-lastSend);
            if(wait>0){if(flushTimer===null){flushTimer=win.setTimeout(function(){flushTimer=null;flush();},wait);}return;}
            const body=queue.shift();flightBody=body;try{flight=send({type:'view.set',connection_epoch:hello.connection_epoch,view_id:id,base_state_rev:state.state_rev,body:body});lastSend=now();
                editTimer=win.setTimeout(function(){editTimer=null;lost('Edit outcome unknown; checking current view without replay.');},10000);updateCursor();}
            catch(_){lost('Input not confirmed; reconnecting without replay.');}}
        function edit(body,done){if(!ready()||queue.length>=8){if(done){done('Input was not applied.');}status('Input unavailable; wait for this view.');return;}
            if(done){callbacks.set(body,done);}if(!body.navigation||body.navigation.kind!=='pan'||!body.navigation.snap){freezeDisplay();}queue.push(body);
            if(image){el('frame-status').textContent='Previous image · waiting for current frame';}flush();updateCursor();}
        function incoming(event,token){if(token!==serial||stopped||hidden){return;}
            received=now();
            try{if(typeof event.data!=='string'){frame(event.data,token);return;}if(event.data.length>256*1024){throw Error('Control limit');}const v=JSON.parse(event.data);
                if(v.type==='hello'){if(hello||v.protocol!==1||v.bundle!==bundle||v.view_id!==id||!/^[0-9a-f]{64}$/.test(v.connection_epoch)||
                    v.frame_credit!==1||!v.capabilities||v.capabilities.view!==true||['index','review','export','query'].some(function(k){return v.capabilities[k]!==false;})){throw Error('Invalid handshake');}
                    hello=v;connectTimer=timerClear(connectTimer);delay=500;controls();return;}
                if(!hello){throw Error('Missing handshake');}
                if(v.type==='opening'){if(v.view_id!==id||v.connection_epoch!==hello.connection_epoch){throw Error('Wrong opening identity');}status('Opening assigned layout…');updateCursor();return;}
                if(v.type==='snapshot'){validate(v);
                    const b=v.bbox_dbu.map(Number),dbu=Number(v.dbu_um);
                    if(Number.isFinite(dbu)){el('viewport-info').textContent=((b[2]-b[0])*dbu).toPrecision(6)+' × '+((b[3]-b[1])*dbu).toPrecision(6)+' µm';}
                    el('dstatus').textContent='depth: '+v.depth+(v.max_depth==null?'':'/'+v.max_depth)+' · detail: '+v.detail+' · thin:'+(v.effective_thin||v.thin)+' · frame:'+(v.frames?'on':'off');if(state&&state.state_rev!==v.state_rev){if(gesture){gesture.cancel();}if(image&&!P.placement(image.header,v)){freezeDisplay();}releasedPan=false;shift=null;}state=v;
                    if(v.failure){rejectQueue();}viewControls.sync();compose();controls();settle();resized();status(v.failure?'Renderer failed; end this session and request a new launch.':'Connected · '+v.status+(v.root_name?' · root '+v.root_name:''));return;}
                if(v.type==='accepted'){if(!flight||v.seq!==flight){throw Error('Wrong acknowledgment');}P.counter(v.state_rev);flight=null;accepted=v.state_rev;settle();return;}
                if(v.type==='error'){if(!flight||v.seq!==flight){throw Error('Wrong error response');}flight=null;rejectQueue();finishEdit(flightBody,'View edit rejected.');flightBody=null;accepted=null;releasedPan=false;shift=null;restorePixels();compose();editTimer=timerClear(editTimer);status('View edit rejected; check current state before trying again.');return;}
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
        const viewControls=V.bindControls({el:el,document:doc,protocol:P,edit:edit,notice:status,now:now,gesture:function(){return gesture;},
            context:function(){return {id:id,state:state,ready:ready()};}}),nav=viewControls.navigate;
        // The cell tree's keys (t, Ctrl+T, Ctrl+Shift+T, Escape) come first,
        // except while a gesture owns Escape.
        port.addEventListener('keydown',function(e){
            if(cells&&!e.isComposing&&e.keyCode!==229&&!e.metaKey&&!e.altKey&&!(gesture&&gesture.active())){const k=V.keyName(e);
                if((k==='t'||k==='T')&&e.ctrlKey&&!e.repeat||k==='t'&&!e.ctrlKey||k==='Escape'&&!e.ctrlKey){if(cells.key(k,e)){e.preventDefault();return;}}}
            viewControls.key(e);});
        function editable(){return ready()&&!flight&&!accepted&&!queue.length;}
        function paletteContext(){return ready()?{id:id,key:state.render_key,epoch:hello.connection_epoch,rev:state.state_rev,slotKey:state.fill_slots_key,fillEdit:false,connected:true,editable:editable()}:null;}
        function rowStyles(r,scope,valid){
            const color=doc.createElement('input');color.type='color';color.value=r.color;color.className='layer-edit';color.setAttribute('aria-label','Color '+r.name);
            color.onchange=function(){const value=color.value;color.value=r.color;if(!valid()){status('Layer styles changed; select the layer again.');return;}
                edit({style_batch:{pairs:[r.pair],collapsed:r.closed?[r.pair]:[],color:value}});};
            const style=doc.createElement('button');style.type='button';style.className='layer-swatch';style.setAttribute('aria-label','Edit style '+r.name);style.title=r.color+' · '+r.fill.kind+' · '+r.width+' px';
            const swatch=doc.createElement('canvas');swatch.setAttribute('aria-hidden','true');if(o.palette&&o.palette.swatch){o.palette.swatch(swatch,r);}style.appendChild(swatch);
            style.onclick=function(){if(!valid()){return;}palette.closeStyle();rowStyle={row:r,key:scope.key,valid:valid};
                el('style-title').textContent=r.name;el('style-fill').value=r.fill.kind;el('style-width').value=r.width;el('style-color').value=r.color;
                el('style-pattern').value=(r.fill.rows||new Array(16).fill(0xaaaa)).map(function(n){return n.toString(16).padStart(4,'0');}).join(' ');
                el('style-editor').hidden=false;patternControls();el('style-fill').focus();};
            return {color:color,style:style};
        }
        function patternControls(){el('style-pattern').hidden=el('pattern-label').hidden=el('style-fill').value!=='pattern';}
        function closeRowStyle(){rowStyle=null;el('style-editor').hidden=true;}
        if(o.palette){
            el('style-fill').onchange=patternControls;el('style-cancel').onclick=closeRowStyle;
            el('style-editor').onsubmit=function(event){event.preventDefault();
                if(!rowStyle||!state||rowStyle.key!==state.render_key||!rowStyle.valid()){status('Layer styles changed; select the layer again.');closeRowStyle();return;}
                const fill={kind:el('style-fill').value},width=Number(el('style-width').value),row=rowStyle.row,delta={pairs:[row.pair],collapsed:row.closed?[row.pair]:[]};
                if(!Number.isInteger(width)||width<1||width>8){status('Line width must be 1–8 device pixels.');return;}
                if(fill.kind==='pattern'){const rows=el('style-pattern').value.trim().split(/\s+/);
                    if(rows.length!==16||!rows.every(function(t){return /^[0-9a-f]{4}$/i.test(t);})){status('A pattern needs exactly 16 four-digit hex rows.');return;}
                    fill.rows=rows.map(function(t){return parseInt(t,16);});}
                const color=String(el('style-color').value||'').toLowerCase();
                if(/^#[0-9a-f]{6}$/.test(color)&&color!==String(row.color).toLowerCase()){delta.color=color;}
                if(fill.kind!==row.fill.kind||(fill.kind==='pattern'&&fill.rows.some(function(n,i){return n!==row.fill.rows[i];}))){delta.fill=fill;}
                if(width!==row.width){delta.width=width;}
                if(delta.fill||delta.color||delta.width!==undefined){edit({style_batch:delta});}closeRowStyle();};
            palette=o.palette.bind({el:el,document:doc,window:win,http:http,edit:edit,styles:rowStyles,presets:o.presets,slotEditor:o.fillEditor,wholeList:true,
                editSlot:function(c,body,done){done('Bitmap editing is not available in this session.');return null;},
                closeRowStyle:closeRowStyle,painted:function(){},context:paletteContext});
        }
        if(o.minimap){minimap=o.minimap.bind({el:el,document:doc,http:http,state:function(){return ready()?state:null;},
            ready:function(){return editable()&&!(gesture&&gesture.active());},navigate:function(n){nav(n);},focus:function(){port.focus();}});}
        if(o.panes){panes=o.panes.bind({el:el,document:doc,window:win,focus:function(){port.focus();},resized:resized,
            changed:function(){if(menubar){menubar.refresh();}},blocked:function(){return stopped;}});}
        if(o.cells){cells=o.cells.bind({el:el,document:doc,http:http,edit:edit,
            context:function(){return ready()?{id:id,state:state,connected:true}:null;},
            size:function(){return state?screen():null;},unit:function(){return state?Number(state.dbu_um):1;},
            focus:function(){port.focus();},raise:function(){if(panes){panes.raise('left','cells-page');}},
            rootAllowed:function(){return !!(state&&state.capabilities&&state.capabilities.cell_root);},buildAllowed:function(){return false;},buildOffered:false,
            now:now,setTimeout:function(f,ms){return win.setTimeout(f,ms);},clearTimeout:function(t){win.clearTimeout(t);}});}
        if(o.menubar){
            function proxy(label,name,key){return {label:label,proxy:name,key:key};}
            menubar=o.menubar.bind({el:el,document:doc,window:win,focus:function(){port.focus();},report:function(e){status(String(e&&e.message||e));},model:[
                {name:'File',items:[
                    {label:'Samples…',hidden:function(){return !publicDemo;},action:function(){o.location.href='/demo';}},
                    proxy('Reconnect','reconnect'),{sep:true},proxy('End my session','leave')]},
                {name:'View',items:[
                    proxy('Fit (zoom all)','fit','Ctrl+A'),
                    {label:'Zoom in 50%',key:'Ctrl+Z',enabled:function(){return !el('fit').disabled;},action:function(){nav({kind:'zoom',factor:0.5});}},
                    {label:'Zoom out 50%',key:'Shift+Z',enabled:function(){return !el('fit').disabled;},action:function(){nav({kind:'zoom',factor:2});}},
                    {label:'Goto position…',key:'g',enabled:function(){return !el('goto').disabled;},action:function(){el('goto-x').focus();el('goto-x').select();}},
                    {sep:true},
                    {label:'Detail',key:'d',sub:[{label:'Low · 5 px',radio:{id:'detail',value:'low'}},{label:'Medium · 3 px',radio:{id:'detail',value:'medium'}},{label:'High · 1 px',radio:{id:'detail',value:'high'}},{label:'Exact · no cut',radio:{id:'detail',value:'exact'}}]},
                    {label:'Label size…',enabled:function(){return !el('font-px').disabled;},action:function(){if(panes){panes.show('display');}el('font-px').focus();el('font-px').select();}},
                    {label:'Depth +1',key:'>',enabled:function(){return !el('depth').disabled;},action:function(){edit({depth_step:1});}},
                    {label:'Depth −1',key:'<',enabled:function(){return !el('depth').disabled;},action:function(){edit({depth_step:-1});}},
                    {label:'Depth full',key:'9 9',enabled:function(){return !el('depth').disabled;},action:function(){edit({depth:'full'});}},
                    {sep:true},
                    {label:'Hierarchy frames',key:'f',toggle:'frames'},{label:'Labels',toggle:'labels'},{label:'Grayscale layers',key:'b',toggle:'mono'},
                    {label:'Thin shapes at wide views',sub:[{label:'Auto (keep)',radio:{id:'thin',value:'auto'}},{label:'Keep (thin shapes as hairlines)',radio:{id:'thin',value:'keep'}},{label:'Cull (drop all-thin pages, faster)',radio:{id:'thin',value:'cull'}}]},
                    {sep:true},
                    {label:'Layers',sub:[proxy('Show all','layers-all'),proxy('Hide all','layers-none'),{sep:true},proxy('Show selected','layers-show'),proxy('Hide selected','layers-hide'),proxy('Toggle selected','layers-toggle'),proxy('Style selected…','layers-style'),proxy('Clear selection','layers-clear'),{sep:true},proxy('Expand all groups','layers-expand'),proxy('Collapse all groups','layers-collapse')]},
                    {sep:true},
                    {label:'Display options…',action:function(){if(panes){panes.show('display');}}}]},
                {name:'Cell',items:[
                    {label:'Cell tree / find cell…',key:'t',enabled:function(){return !el('cells-search').disabled;},action:function(){if(panes){panes.raise('left','cells-page');}el('cells-search').focus();}},
                    proxy('Zoom to selected cell','cells-zoom','Enter'),
                    {label:'Highlight instances',toggle:'cells-highlight'},
                    {label:'Clear highlight',key:'Esc',enabled:function(){return !!cells&&cells.hasSelection();},action:function(){cells.clearHighlight();}},
                    {sep:true},
                    proxy('Selected cell as view root','cells-root','Ctrl+T'),proxy('View root: back to the top cell','cells-top','Ctrl+Shift+T')]}
            ]});
        }
        function screen(){const r=port.getBoundingClientRect();return V.screen(r,state.pixels,V.dimensions(P,r,win.devicePixelRatio||1));}
        if(o.gestures){gesture=o.gestures.bind({window:win,document:doc,viewport:port,ready:function(){return ready()&&!!image&&P.matches(image.header,state)&&!flight&&!accepted&&!queue.length;},
            stamp:function(){return state&&hello.connection_epoch+':'+state.state_rev;},dimensions:screen,
            requestAnimationFrame:win.requestAnimationFrame.bind(win),cancelAnimationFrame:win.cancelAnimationFrame.bind(win),
            preview:function(p,paint){if(!paint){freezeDisplay();}shift=p;releasedPan=!paint;if(paint){compose();}},
            band:nav,bandReady:function(){return !!image&&P.matches(image.header,state);},notice:status,
            bandPreview:function(b){V.band(el,b);},cursor:updateCursor,pan:nav});
            port.addEventListener('wheel',function(e){V.wheel({protocol:P,gestures:o.gestures,viewport:port,navigate:nav,context:function(){
                const c=viewerContext();c.active=c.active&&ready();c.size=state?screen():null;
                if(state&&dimensions().some(function(n,i){return n!==state.pixels[i];})){c.size=null;}return c;
            }},e);},{passive:false});}
        function pause(){hidden=true;disconnect();status('Hidden · disconnected. Reconnecting does not replay input.');}
        function resume(){hidden=!!doc.hidden;if(!hidden&&!stopped){join();}}
        doc.addEventListener('visibilitychange',function(){if(doc.hidden){pause();}else{resume();}});
        win.addEventListener('pagehide',pause);win.addEventListener('pageshow',resume);win.addEventListener('resize',resized);
        controls();return {start:start,stop:function(){stop('Session display closed.',false);}};
    }
    if(typeof module==='object'&&module.exports){module.exports={bind:bind};}
    else{const w=root,c=bind({window:w,document:w.document,location:w.location,history:w.history,protocol:w.FloeProtocol,viewer:w.FloeViewer,gestures:w.FloeGestures,
        palette:w.FloePalette,presets:w.FloePresets,fillEditor:w.FloeFillEditor,minimap:w.FloeMinimap,panes:w.FloePanes,menubar:w.FloeMenubar,cells:w.FloeCells,
        XHR:w.XMLHttpRequest,WebSocket:w.WebSocket,decode:function(h,d,done){return w.FloeImageDecode.create(w,h,d,done);}});c.start();}
}(typeof window==='object'?window:this));
