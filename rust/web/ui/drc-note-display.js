/* ES2017 saved-note display: bounded reads, never edit capabilities or writes.
 * One immutable page/focus result. Pan does not change the request identity. */
(function(root) {
    'use strict';
    function fail(){throw new Error('Invalid saved-note display response');}
    function keys(v,names){if(!v||typeof v!=='object'||Array.isArray(v)||Object.keys(v).length!==names.length||names.some(function(k){return !Object.prototype.hasOwnProperty.call(v,k);})){fail();}}
    function ref(v,P){keys(v,['check','error']);P.counter(v.check,true);P.counter(v.error,true);}
    function same(a,b){return !!a&&!!b&&a.check===b.check&&a.error===b.error;}
    function validate(v,s,P){
        keys(v,['kind','context','review_rev','reviewer','name','rows','focus','exists','legacy_unverified','cache_hit','import_report']);
        keys(v.context,['drc_id','revision','view_id']);
        if(v.kind!=='drc_note_display'||Object.keys(v.context).some(function(k){return v.context[k]!==s.body.context[k];})||
            v.review_rev!==s.review_rev||v.reviewer!==s.reviewer||typeof v.name!=='string'||v.name.length>4096||
            ['exists','legacy_unverified','cache_hit'].some(function(k){return typeof v[k]!=='boolean';})){fail();}
        P.counter(v.review_rev,true);
        if(!Array.isArray(v.rows)||v.rows.length!==s.body.errors.length||v.rows.length>512){fail();}
        v.rows.forEach(function(r,i){keys(r,['check','error','noted']);ref({check:r.check,error:r.error},P);if(!same(r,s.body.errors[i])||typeof r.noted!=='boolean'){fail();}});
        if(s.body.focus){keys(v.focus,['check','error','text']);if(!same(v.focus,s.body.focus)||v.focus.text!==null&&
            (typeof v.focus.text!=='string'||new TextEncoder().encode(v.focus.text).length>65536)){fail();}}
        else if(v.focus!==null){fail();}
        keys(v.import_report,['skipped_lines','invalid_members','reassigned_members']);
        Object.keys(v.import_report).forEach(function(k){if(!Number.isSafeInteger(v.import_report[k])||v.import_report[k]<0){fail();}});
        return v;
    }
    function bind(o){
        let desired=null,key='',serial=0,running=null,result=null,message='',stopped=false,scheduled=false,layout=null;
        function emit(){o.changed();}
        function sync(force){
            const s=stopped?null:o.source(),next=s?JSON.stringify(s):'';
            if(!force&&next===key){return;}
            key=next;++serial;desired=s;result=null;layout=null;
            message=!s?'':s.blocked||(!s.body.errors.length&&!s.body.focus?'Choose an error page to see saved-note badges.':'Loading saved notes…');
            emit();queue();
        }
        function queue(){if(scheduled||running||stopped){return;}scheduled=true;Promise.resolve().then(function(){scheduled=false;run();});}
        async function run(){
            const s=desired;if(running||stopped||!s||s.blocked||(!s.body.errors.length&&!s.body.focus)){return;}
            const t={cancelled:false,abort:null,serial:serial};running=t;
            try{const v=validate(await o.http('POST','/api/v1/drc/review/notes/display',s.body,false,t),s,o.protocol);
                if(stopped||t.cancelled||serial!==t.serial){return;}
                result=v;const loss=Object.keys(v.import_report).filter(function(k){return v.import_report[k]>0;}).map(function(k){return k+': '+v.import_report[k];});
                message='Saved notes · '+v.reviewer+' · revision '+v.review_rev+(v.legacy_unverified?' · legacy binding UNVERIFIED':'')+
                    (loss.length?' · import loss: '+loss.join(', '):'');emit();
            }catch(e){if(!stopped&&!t.cancelled&&serial===t.serial){message='Saved notes unavailable · '+(e.code||e.message)+'. Refresh to retry; external file changes require Reload snapshot or reopening the review.';emit();}}
            finally{running=null;if(serial!==t.serial){queue();}}
        }
        function text(){return result&&result.focus?result.focus.text:null;}
        function paint(ctx,size){
            const value=text();if(!value){return;}
            const d=size.dpr,w=size.pixels[0]/d,h=size.pixels[1]/d;
            if(w<80||h<48){return;}
            const width=Math.min(340,w-40),lines=Math.max(1,Math.floor((h-44)/16));
            ctx.save();ctx.font=(12*d)+'px sans-serif';ctx.textBaseline='top';
            if(!layout||layout.value!==value||layout.width!==width||layout.lines!==lines||layout.d!==d){
                const out=[];let line='',units=0,clipped=false;
                // Stop measuring once the visible prefix is full; retain the
                // entire text only in the read-only panel, not in storage.
                for(const ch of value){
                    if(ch==='\r'){continue;}
                    // Zero-width/combining runs must not grow the measured
                    // prefix without bound (quadratic text shaping).
                    if(ch==='\n'||units>=128||ctx.measureText(line+ch).width>width*d){out.push(line);line='';units=0;if(out.length>=lines){clipped=true;break;}}
                    if(ch!=='\n'){line+=ch;++units;}
                }
                if(!clipped){out.push(line);}else{out[out.length-1]='… note clipped; full text in panel';}
                layout={value:value,width:width,lines:lines,d:d,out:out};
            }
            ctx.fillStyle='rgba(0,0,0,0.6)';ctx.fillRect(10*d,10*d,(width+20)*d,(layout.out.length*16+8)*d);
            ctx.beginPath();ctx.rect(10*d,10*d,(width+20)*d,(layout.out.length*16+8)*d);ctx.clip();ctx.fillStyle='#f0f0f0';
            layout.out.forEach(function(line,i){ctx.fillText(line,20*d,(14+i*16)*d);});ctx.restore();
        }
        return {sync:sync,text:text,paint:paint,message:function(){return message;},visible:function(){return !!desired;},
            noted:function(r){const row=result&&result.rows.find(function(v){return same(v,r);});return row?row.noted:null;},
            stop:function(){stopped=true;if(running){running.cancelled=true;if(running.abort){running.abort();}}sync(true);},
            resume:function(){stopped=false;sync(true);}};
    }
    const api={bind:bind,validate:validate};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDRCNoteDisplay=api;}
}(typeof window==='object'?window:this));
