'use strict';
// Synthetic QA only. No URL/hash contents, storage, cookies or page text cross
// this boundary. Bits distinguish a hidden document from incomplete exchange.
const probe = `(()=>{
  const b=document.getElementById('logout');
  return (!document.hidden?1:0)|(!location.hash?2:0)|(b?4:0)|(b&&!b.disabled?8:0);
})()`;
async function waitReady({ evaluate, alive, observe, timeout = 30000 }) {
  let expired = false, timer;
  const deadline = performance.now() + timeout;
  const limit = new Promise((_, reject) => {
    timer = setTimeout(() => { expired = true; reject(Error('Synthetic readiness deadline')); }, timeout);
  });
  const poll = async () => {
    while (!expired && alive() && performance.now() < deadline) {
      const raw = await evaluate(probe);
      if (expired || !alive() || performance.now() >= deadline) break;
      const bits = Number.isInteger(raw) && raw >= 0 && raw <= 15 ? raw : null;
      observe(bits);
      if (bits === 15) return;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw Error('Synthetic readiness incomplete');
  };
  try { await Promise.race([poll(), limit]); }
  finally { expired = true; clearTimeout(timer); }
}
module.exports = { probe, waitReady };
