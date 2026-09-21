'use strict';
// Explicit OS clipboard overwrite test, never run by the ordinary smoke gate.
// No original clipboard read/backup/restore. Read only after a test write succeeds.
const { clipboard, nativeImage } = require('electron');
async function run({ window, evalOwned, wait, stage }) {
  const web = window.webContents;
  window.show(); window.focus(); web.focus();
  await wait("!document.hidden&&!document.getElementById('snapshot-copy').disabled&&/^Live/.test(document.getElementById('status').textContent)&&document.getElementById('rendering').hidden");
  const text = 'floe2 synthetic clipboard · 한글 012345';
  const literal = JSON.stringify(text);
  async function click(id) {
    const pos = await evalOwned('(()=>{const b=document.getElementById(' + JSON.stringify(id) + ');b.scrollIntoView({block:"center"});const r=b.getBoundingClientRect();return {x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)};})()');
    web.sendInputEvent({ type: 'mouseDown', button: 'left', clickCount: 1, ...pos });
    web.sendInputEvent({ type: 'mouseUp', button: 'left', clickCount: 1, ...pos });
  }
  stage('clipboard unactivated write denial');
  if (await evalOwned('navigator.clipboard.writeText(' + literal + ').then(()=>false,()=>true)') !== true) {
    throw new Error('Unactivated clipboard write was allowed');
  }
  stage('clipboard trusted text copy');
  await evalOwned(`(()=>{
    const b=document.createElement('button');b.id='synthetic-copy-text';b.textContent='Copy synthetic text';
    b.style.cssText='position:fixed;top:20px;left:20px;z-index:99999';
    b.onclick=e=>{b.dataset.trusted=String(e.isTrusted);navigator.clipboard.writeText(${literal}).then(()=>{b.dataset.result='written';},()=>{b.dataset.result='denied';});};
    document.body.appendChild(b);return true;
  })()`);
  await click('synthetic-copy-text');
  await wait("!!document.getElementById('synthetic-copy-text').dataset.result");
  const textFlags = await evalOwned("(()=>{const e=document.getElementById('synthetic-copy-text');return {trusted:e.dataset.trusted==='true',written:e.dataset.result==='written',focus:document.hasFocus(),active:navigator.userActivation.isActive};})()");
  console.log('ELECTRON CLIPBOARD: text flags ' + JSON.stringify(textFlags));
  if (!textFlags.written) throw new Error('Synthetic text copy denied');
  const textMatch = await clipboard.readText() === text;
  console.log('ELECTRON CLIPBOARD: native text matches ' + textMatch);
  if (!textMatch) throw new Error('Just-written synthetic text mismatch');
  if (await evalOwned("document.getElementById('synthetic-copy-text').dataset.trusted==='true'") !== true) throw new Error('Copy input was not trusted');
  console.log('ELECTRON CLIPBOARD: trusted text copy matched (original clipboard not read)');

  stage('clipboard programmatic read denial');
  // These calls occur only AFTER the known test text was written. No content is
  // returned/logged even if a policy regression accidentally allows the read.
  for (const method of ['readText', 'read']) {
    if (await evalOwned('navigator.clipboard.' + method + '().then(()=>false,()=>true)') !== true) throw new Error('Clipboard read was allowed');
  }
  stage('clipboard native text copy and paste');
  const nativeText = text + ' native', nativeLiteral = JSON.stringify(nativeText);
  await evalOwned(`(()=>{
    const b=document.getElementById('synthetic-copy-text');b.remove();
    const input=document.createElement('textarea');input.id='synthetic-clipboard-text';
    input.value=${nativeLiteral};input.addEventListener('copy',()=>{input.dataset.copied='yes';});
    document.body.appendChild(input);input.focus();input.select();return true;
  })()`);
  web.copy(); // native Edit Copy path; only this synthetic selection
  await wait("document.getElementById('synthetic-clipboard-text').dataset.copied==='yes'");
  if (await clipboard.readText() !== nativeText) throw new Error('Native synthetic copy mismatch');
  await evalOwned("(()=>{const e=document.getElementById('synthetic-clipboard-text');e.value='';e.focus();return true;})()");
  web.paste(); // only the test value just written above
  await wait('document.getElementById("synthetic-clipboard-text").value===' + nativeLiteral);
  await evalOwned("document.getElementById('synthetic-clipboard-text').remove();true");

  let copiedSize;
  for (const kind of ['button', 'shortcut']) {
    stage('clipboard visible PNG copy ' + kind);
    await wait("!document.getElementById('snapshot-copy').disabled&&document.getElementById('rendering').hidden&&/^Live/.test(document.getElementById('status').textContent)");
    // QA-only observation of the encoder callback: retain the exact synthetic
    // blob while forwarding it unchanged to the product. No renderer native bridge.
    await evalOwned(`(()=>{
      const original=HTMLCanvasElement.prototype.toBlob;
      HTMLCanvasElement.prototype.toBlob=function(callback,...args){
        HTMLCanvasElement.prototype.toBlob=original;
        return original.call(this,blob=>{
          window.__floeQaPng=blob?blob.arrayBuffer().then(b=>Array.from(new Uint8Array(b))):Promise.resolve([]);
          callback(blob);
        },...args);
      };return true;
    })()`);
    if (kind === 'button') await click('snapshot-copy');
    else {
      await evalOwned("window.getSelection().removeAllRanges();document.getElementById('viewport').focus();true");
      const modifiers = [process.platform === 'darwin' ? 'meta' : 'control'];
      web.sendInputEvent({ type: 'keyDown', keyCode: 'c', modifiers });
      web.sendInputEvent({ type: 'keyUp', keyCode: 'c', modifiers });
    }
    await wait("!!window.__floeQaPng&&/^Copied [0-9]+ × [0-9]+ device pixels/.test(document.getElementById('snapshot-status').textContent)");
    stage('clipboard native PNG read ' + kind);
    const items = await clipboard.read(); // only after the product reports copy success
    if (items.length !== 1 || !items[0].types.includes('image/png')) throw new Error('Synthetic PNG copy missing');
    const png = await items[0].getType('image/png');
    const image = nativeImage.createFromBuffer(Buffer.from(await png.arrayBuffer()));
    if (image.isEmpty()) throw new Error('Synthetic PNG copy empty');
    const size = image.getSize(), bitmap = image.toBitmap();
    const expected = await evalOwned("(()=>{const s=document.getElementById('snapshot-status').textContent.match(/^Copied ([0-9]+) × ([0-9]+) device pixels/);return [Number(s[1]),Number(s[2])];})()");
    if (size.width !== expected[0] || size.height !== expected[1] || bitmap.length !== size.width * size.height * 4) throw new Error('Synthetic PNG dimensions mismatch');
    stage('clipboard frozen PNG pixel comparison ' + kind);
    const encoded = await evalOwned('window.__floeQaPng');
    await evalOwned('delete window.__floeQaPng;true');
    const frozen = nativeImage.createFromBuffer(Buffer.from(encoded));
    if (frozen.isEmpty() || !frozen.toBitmap().equals(bitmap)) throw new Error('Synthetic clipboard PNG pixel mismatch');
    let lit = 0;
    for (let i = 0; i < bitmap.length; i += 4) if (bitmap[i] || bitmap[i + 1] || bitmap[i + 2]) lit++;
    if (!lit) throw new Error('Synthetic PNG copy blank');
    copiedSize = size;
    console.log('ELECTRON CLIPBOARD: PNG ' + kind + ' exact bitmap matched');
  }
  stage('clipboard grant does not persist after activation');
  await wait('navigator.userActivation.isActive===false');
  if (await evalOwned('navigator.clipboard.writeText(' + literal + ').then(()=>false,()=>true)') !== true) throw new Error('Clipboard grant persisted');
  console.log('ELECTRON CLIPBOARD: OK (trusted text write; native text copy/paste; reads and unactivated writes denied; button and injected shortcut PNG ' + copiedSize.width + 'x' + copiedSize.height + ' exact bitmap; original not read/restored; test PNG remains)');
}
module.exports = { run };
