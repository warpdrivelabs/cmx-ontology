'use strict';
// 验证 <cmx-ontology-graph> content 图组件 light/dark 主题跟随门户 --sap* 令牌翻转（零 JS）。
const { chromium } = require('playwright');
const http = require('http');
const ONTO = { host: '127.0.0.1', port: 8097 }, KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6', PORT = 9170;
function srv() { return new Promise(res => { const s = http.createServer((req, rq) => { const u = req.url.split('?')[0]; if (u === '/') { rq.setHeader('Content-Type', 'text/html; charset=utf-8'); rq.end(H); return; } if (u.startsWith('/api/')) { const c = []; req.on('data', x => c.push(x)); req.on('end', () => { const b = c.length ? Buffer.concat(c) : null; const o = { hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}`, 'x-api-key': KEY } }; const p = http.request(o, pr => { rq.writeHead(pr.statusCode, pr.headers); pr.pipe(rq); }); p.on('error', () => { rq.writeHead(502); rq.end(); }); if (b) p.write(b); p.end(); }); return; } rq.statusCode = 404; rq.end(); }); s.listen(PORT, () => res(s)); }); }
// Horizon light/dark 代表性 --sap* 值（注入 documentElement）。
const SAP = {
  light: { '--sapBackgroundColor': '#f5f6f7', '--sapList_Background': '#ffffff', '--sapTextColor': '#1c2530', '--sapContent_LabelColor': '#5a6b7b', '--sapList_BorderColor': '#c9ced4', '--sapHighlightColor': '#0a6ed1' },
  dark: { '--sapBackgroundColor': '#12171c', '--sapList_Background': '#1d232a', '--sapTextColor': '#eaecee', '--sapContent_LabelColor': '#a9b4be', '--sapList_BorderColor': '#3a4149', '--sapHighlightColor': '#4db1ff' },
};
const H = `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;height:100%}#stage{display:grid;grid-template-columns:230px 1fr 340px;grid-template-rows:52px 1fr;height:100vh}#r-model{grid-column:1/4}.region{overflow:auto;height:100%}.host{height:100%;display:block}</style></head>
<body><div id="stage"><div class="region" id="r-model"><div class="host" id="h-model"></div></div><div class="region" id="r-explorer"><div class="host" id="h-explorer"></div></div><div class="region" id="r-content"><div class="host" id="h-content"></div></div><div class="region" id="r-property"><div class="host" id="h-property"></div></div></div>
<script type="module">
globalThis.__cmxDataComp={apiJson:async(url,options,CFG)=>{const full=(CFG&&CFG.apiBase&&url[0]==='/')?CFG.apiBase+url:url;const r=await fetch(full,{...((CFG&&CFG.fetchInit)||{}),...(options||{}),headers:{Accept:'application/json',...((CFG&&CFG.authHeaders&&CFG.authHeaders())||{}),...((options&&options.headers)||{})}});let j=null;try{j=await r.json()}catch{}if(!r.ok||(j&&typeof j.code==='number'&&j.code!==0))throw new Error((j&&(j.msg||j.error))||('HTTP '+r.status));return j&&typeof j==='object'&&'data'in j?j.data:j},escHtml:(s)=>String(s==null?'':s).replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]))};
const s=await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json());const src=s.data?s.data.source:s.source;const mod=await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'})));mod.configure({apiBase:''});const d=mod.default;
await d.views.content({host:document.getElementById('h-content')});window.__ready=true;
</script></body></html>`;
const GS = `function gs(){const st=[document];while(st.length){const r=st.pop();const el=r.querySelector&&r.querySelector('cmx-ontology-graph');if(el&&el.shadowRoot)return el.shadowRoot;const all=r.querySelectorAll?r.querySelectorAll('*'):[];for(const e of all){if(e.shadowRoot)st.push(e.shadowRoot)}}return null}`;
const clickKey = k => `(()=>{${GS};const s=gs();const g=s&&s.querySelector('[data-group-toggle="${k}"]');if(g){g.dispatchEvent(new MouseEvent('click',{bubbles:true}));return true}return false})()`;
function setTheme(vars) { let css = ':root{'; for (const k in vars) css += k + ':' + vars[k] + ';'; css += '}'; let el = document.getElementById('__sap'); if (!el) { el = document.createElement('style'); el.id = '__sap'; document.head.appendChild(el); } el.textContent = css; }
let _p = 0, _t = 0; function A(id, ok, d, x) { _t++; if (ok) _p++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${d}${x ? '  :: ' + x : ''}`); }
(async () => {
  const s = await srv(); const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1000, height: 760 } });
  try {
    await p.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' }); await p.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    await p.waitForFunction(`(()=>{${GS};const s=gs();return !!(s&&(s.querySelector('.og-grp')||s.querySelector('.og-object')||s.querySelector('.og-canvas')))})()`, { timeout: 15000 }).catch(()=>{});
    // 读组件 canvas 背景色 computed
    const bgOf = `(()=>{${GS};const s=gs();const c=s&&s.querySelector('.og-canvas');return c?getComputedStyle(c).backgroundColor:'?';})()`;
    const fgOf = `(()=>{${GS};const s=gs();const t=s&&(s.querySelector('.og-title')||s.querySelector('.og-grp-label'));return t?getComputedStyle(t).fill:'?';})()`;
    // light
    await p.evaluate(setTheme, SAP.light); await p.waitForTimeout(200);
    const bgL = await p.evaluate(bgOf); const fgL = await p.evaluate(fgOf);
    await p.screenshot({ path: require('path').join(__dirname, 'shots', 'onto_graph_theme_light.png') }).catch(() => {});
    // dark
    await p.evaluate(setTheme, SAP.dark); await p.waitForTimeout(200);
    const bgD = await p.evaluate(bgOf); const fgD = await p.evaluate(fgOf);
    await p.screenshot({ path: require('path').join(__dirname, 'shots', 'onto_graph_theme_dark.png') }).catch(() => {});
    console.log('light bg=', bgL, 'fg=', fgL);
    console.log('dark  bg=', bgD, 'fg=', fgD);
    // light bg 应亮(接近白 #f5..#ff)、dark bg 应暗(#12..#1d)；两者不同 = 翻转成功
    const isLight = c => { const m = c.match(/[0-9]+/g); return m && (+m[0] + +m[1] + +m[2]) / 3 > 160; };
    A('bg-flips', bgL !== bgD, `canvas 背景随主题变 (light=${bgL} dark=${bgD})`);
    A('light-is-light', isLight(bgL), `light 主题背景为亮色 ${bgL}`);
    A('dark-is-dark', !isLight(bgD), `dark 主题背景为暗色 ${bgD}`);
    A('fg-flips', fgL !== fgD, `文本色随主题变 (light=${fgL} dark=${fgD})`);
    console.log(`\n主题 CDP：${_p}/${_t} 通过`);
    await b.close(); s.close(); process.exit(_p === _t ? 0 : 1);
  } catch (e) { console.log('FATAL', e.message); await b.close(); s.close(); process.exit(1); }
})();
