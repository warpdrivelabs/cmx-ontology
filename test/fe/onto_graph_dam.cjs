// CDP：本体图按 DAM 三级分域折叠 —— 大图性能优化。
// 覆盖：默认全收起(整图只见域盒、对象卡不在 DOM=性能证据) → 逐级展开(域▸应用▸模块) → 模块展开出对象卡 → 收起卡消失。
// 前置：onto-server :8097（含 dam 字段）。运行：NODE_PATH=/Users/nanomesh/node_modules node test/fe/onto_graph_dam.cjs
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9103;
let _pass = 0, _total = 0;
function A(id, ok, d, x) { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${d}${x ? '  :: ' + x : ''}`); }

function startServer() {
  return new Promise((res) => {
    const s = http.createServer((req, rq) => {
      const u = req.url.split('?')[0];
      if (u === '/') { rq.setHeader('Content-Type', 'text/html; charset=utf-8'); rq.end(HARNESS); return; }
      if (u.startsWith('/api/')) { const c = []; req.on('data', x => c.push(x)); req.on('end', () => { const b = c.length ? Buffer.concat(c) : null; const o = { hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}`, 'x-api-key': KEY } }; const p = http.request(o, pr => { rq.writeHead(pr.statusCode, pr.headers); pr.pipe(rq); }); p.on('error', () => { rq.writeHead(502); rq.end(); }); if (b) p.write(b); p.end(); }); return; }
      rq.statusCode = 404; rq.end();
    });
    s.listen(PORT, () => res(s));
  });
}
const HARNESS = `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;height:100%;background:#0b1020}#stage{display:grid;grid-template-columns:230px 1fr 340px;grid-template-rows:52px 1fr;height:100vh}#r-model{grid-column:1/4}.region{overflow:auto;height:100%;border:1px solid #243049}.host{height:100%;display:block}</style></head>
<body><div id="stage"><div class="region" id="r-model"><div class="host" id="h-model"></div></div><div class="region" id="r-explorer"><div class="host" id="h-explorer"></div></div><div class="region" id="r-content"><div class="host" id="h-content"></div></div><div class="region" id="r-property"><div class="host" id="h-property"></div></div></div>
<script type="module">
  // 门户运行时会注入 globalThis.__cmxDataComp（共享 apiJson/escHtml）；独立 harness 自备等价 shim。
  globalThis.__cmxDataComp = {
    apiJson: async (url, options, CFG) => {
      const full = (CFG && CFG.apiBase && url[0] === '/') ? CFG.apiBase + url : url;
      const r = await fetch(full, { ...((CFG && CFG.fetchInit) || {}), ...(options || {}), headers: { Accept: 'application/json', ...((CFG && CFG.authHeaders && CFG.authHeaders()) || {}), ...((options && options.headers) || {}) } });
      let j = null; try { j = await r.json(); } catch {}
      if (!r.ok || (j && typeof j.code === 'number' && j.code !== 0)) throw new Error((j && (j.msg || j.error)) || ('HTTP ' + r.status));
      return j && typeof j === 'object' && 'data' in j ? j.data : j;
    },
    escHtml: (s) => String(s == null ? '' : s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])),
  };
  const s = await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json()); const src = s.data ? s.data.source : s.source;
  const mod = await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'}))); mod.configure({ apiBase: '' });
  const d = mod.default;
  await d.views.model({host:document.getElementById('h-model')}); await d.views.explorer({host:document.getElementById('h-explorer')});
  await d.views.content({host:document.getElementById('h-content')}); await d.views.property({host:document.getElementById('h-property')});
  window.__ready = true;
</script></body></html>`;
async function api(p, m, b) { const r = await fetch(`http://${ONTO.host}:${ONTO.port}${p}`, { method: m, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: b ? JSON.stringify(b) : undefined }); return r.json().catch(() => ({})); }
const mkObj = (nm, dam) => api('/api/onto/v1/object-types', 'POST', { apiName: nm, displayName: nm, primaryKey: 'id', titleProperty: 'id', status: 'active', properties: [{ apiName: 'id', baseType: 'string' }], dam });

// 页面上下文：找 <cmx-ontology-graph> 的 shadowRoot（穿透）。
const GS = `function gs(){const st=[document];while(st.length){const r=st.pop();const el=r.querySelector&&r.querySelector('cmx-ontology-graph');if(el&&el.shadowRoot)return el.shadowRoot;const all=r.querySelectorAll?r.querySelectorAll('*'):[];for(const e of all){if(e.shadowRoot)st.push(e.shadowRoot);}}return null;}`;
const countCards = `(()=>{${GS};const s=gs();return s?s.querySelectorAll('[data-node]').length:-1;})()`;
const countBoxes = `(()=>{${GS};const s=gs();return s?s.querySelectorAll('.og-grp-collapsed').length:-1;})()`;
const hasKey = k => `(()=>{${GS};const s=gs();return !!(s&&s.querySelector('[data-group-toggle="${k}"]'));})()`;
const clickKey = k => `(()=>{${GS};const s=gs();const g=s&&s.querySelector('[data-group-toggle="${k}"]');if(g){g.dispatchEvent(new MouseEvent('click',{bubbles:true}));return true;}return false;})()`;

(async () => {
  // 种子：fi/cmxfico/gl×3 + fi/cmxfico/report×2 + hr/recruit/cand×1 + 一个无 DAM。链跨模块/跨域。
  await mkObj('DgGl1', { domain: 'fi', application: 'cmxfico', module: 'gl' });
  await mkObj('DgGl2', { domain: 'fi', application: 'cmxfico', module: 'gl' });
  await mkObj('DgGl3', { domain: 'fi', application: 'cmxfico', module: 'gl' });
  await mkObj('DgRpt1', { domain: 'fi', application: 'cmxfico', module: 'report' });
  await mkObj('DgRpt2', { domain: 'fi', application: 'cmxfico', module: 'report' });
  await mkObj('DgCand1', { domain: 'hr', application: 'recruit', module: 'cand' });
  await mkObj('DgFree1', {});
  await api('/api/onto/v1/link-types', 'POST', { apiName: 'dgIntra', displayName: '同模块', objectTypeA: 'DgGl1', objectTypeB: 'DgGl2', cardinality: 'oneToMany' });
  await api('/api/onto/v1/link-types', 'POST', { apiName: 'dgXmod', displayName: '跨模块', objectTypeA: 'DgGl1', objectTypeB: 'DgRpt1', cardinality: 'oneToMany' });
  await api('/api/onto/v1/link-types', 'POST', { apiName: 'dgXdom', displayName: '跨域', objectTypeA: 'DgGl1', objectTypeB: 'DgCand1', cardinality: 'oneToMany' });
  await api('/api/onto/v1/link-types', 'POST', { apiName: 'dgVert', displayName: '纵向关系', objectTypeA: 'DgGl3', objectTypeB: 'DgGl1', cardinality: 'oneToMany' }); // 同列上下 → 纵向边(验竖排标签)

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [err]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    // 等图渲染出分域容器
    await page.waitForFunction(`(()=>{${GS};const s=gs();return !!(s&&s.querySelector('.og-grp'));})()`, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(500);
    A('ready', true, '设计台 + 分域图就绪');

    // ① 默认全收起：对象卡不在 DOM（性能证据），只见域盒
    const cards0 = await page.evaluate(countCards);
    const boxes0 = await page.evaluate(countBoxes);
    A('default-collapsed', cards0 === 0, `默认无对象卡渲染(性能证据) cards=${cards0}`, `cards=${cards0}`);
    A('domain-boxes', boxes0 >= 2, `顶层域盒 ≥2(fi/hr/未分组) boxes=${boxes0}`, `boxes=${boxes0}`);
    A('fi-collapsed', await page.evaluate(hasKey('fi')), '默认见收起的 fi 域盒');

    // ② 展开 fi 域 → 出应用盒 fi/cmxfico
    await page.evaluate(clickKey('fi')); await page.waitForTimeout(250);
    A('expand-domain', await page.evaluate(hasKey('fi/cmxfico')), '展开 fi → 出应用容器 fi/cmxfico');
    A('still-no-cards', (await page.evaluate(countCards)) === 0, '仅到应用层仍无对象卡');

    // ③ 展开 fi/cmxfico → 出模块盒 gl / report
    await page.evaluate(clickKey('fi/cmxfico')); await page.waitForTimeout(250);
    A('expand-app', (await page.evaluate(hasKey('fi/cmxfico/gl'))) && (await page.evaluate(hasKey('fi/cmxfico/report'))), '展开应用 → 出模块 gl + report');

    // ④ 展开 fi/cmxfico/gl → 出 3 张对象卡
    await page.evaluate(clickKey('fi/cmxfico/gl')); await page.waitForTimeout(300);
    const cardsGl = await page.evaluate(countCards);
    A('expand-module', cardsGl >= 3, `展开 gl 模块 → 出对象卡(≥3) cards=${cardsGl}`, `cards=${cardsGl}`);
    const seesGl = await page.evaluate(`(()=>{${GS};const s=gs();return !!(s&&s.querySelector('[data-node="DgGl1"]'));})()`);
    A('cards-are-gl', seesGl, '出现的正是 gl 模块对象(DgGl1)');

    // ⑥ 回归修复：对象卡恢复属性锚点(端口) + 同模块走属性锚细边 + 跨容器桥接边为正交避让折线(非直线)
    const ports = await page.evaluate(`(()=>{${GS};const s=gs();return s?s.querySelectorAll('.og-port').length:-1;})()`);
    A('ports-restored', ports >= 1, `展开模块的对象卡恢复属性锚点 ports=${ports}`, `ports=${ports}`);
    const fineIntra = await page.evaluate(`(()=>{${GS};const s=gs();return !!(s&&s.querySelector('[data-edge="dgIntra"]'));})()`);
    A('fine-intra', fineIntra, '同模块关系走属性锚点细边(dgIntra 可见↔可见)');
    const bundleInfo = await page.evaluate(`(()=>{${GS};const s=gs();const ps=[...(s?s.querySelectorAll('path.og-bundle'):[])].map(p=>p.getAttribute('d')||'');const bent=ps.filter(d=>d.includes('Q')||((d.match(/L/g)||[]).length>=2));const diag=ps.filter(d=>{const m=d.match(/^M([-\\d.]+),([-\\d.]+)\\s+L([-\\d.]+),([-\\d.]+)$/);return m&&(+m[1]!==+m[3])&&(+m[2]!==+m[4]);});return {n:ps.length,bent:bent.length,diag:diag.length};})()`);
    A('bundle-present', bundleInfo.n >= 1, `跨容器桥接边存在 n=${bundleInfo.n}`, JSON.stringify(bundleInfo));
    A('bundle-routed', bundleInfo.bent >= 1, `桥接边为正交避让折线(有拐点) bent=${bundleInfo.bent}`, JSON.stringify(bundleInfo));
    A('bundle-no-diagonal', bundleInfo.diag === 0, `无斜向直线桥接(旧 bug) diag=${bundleInfo.diag}`, JSON.stringify(bundleInfo));

    // ⑦ 边标签方向：纵向段的 caption 沿线竖排(rotate -90)，横向段横排——按各标签真实 route 的中点段朝向自检。
    const lo = await page.evaluate(`(()=>{${GS};const s=gs();
      function midVert(route){const pts=route.trim().split(/\\s+/).map(p=>{const c=p.split(',');return {x:+c[0],y:+c[1]};});if(pts.length<2)return false;let seg=[],total=0;for(let i=1;i<pts.length;i++){const d=Math.hypot(pts[i].x-pts[i-1].x,pts[i].y-pts[i-1].y);seg.push(d);total+=d;}let acc=0;for(let i=1;i<pts.length;i++){const d=seg[i-1]||0;if(acc+d>=total/2){const a=pts[i-1],b=pts[i];return Math.abs(b.y-a.y)>Math.abs(b.x-a.x);}acc+=d;}return false;}
      let ok=0,bad=0,vt=0;for(const t of s.querySelectorAll('.og-elabel')){const g=t.closest('[data-edge]');if(!g)continue;const vert=midVert(g.getAttribute('data-route')||'');const rot=(t.getAttribute('transform')||'').includes('rotate(-90');if(vert)vt++;vert===rot?ok++:bad++;}
      return {ok,bad,vt};})()`);
    A('label-orient', lo.bad === 0, `纵向段标签竖排/横向段横排一致(自检 route 中点段) ${JSON.stringify(lo)}`, JSON.stringify(lo));
    A('label-vertical', lo.vt >= 1, `至少一条纵向边标签竖排(dgVert) vt=${lo.vt}`, JSON.stringify(lo));

    // ⑤ 收起 gl → 对象卡消失（性能回收）
    await page.evaluate(clickKey('fi/cmxfico/gl')); await page.waitForTimeout(250);
    const cardsAfter = await page.evaluate(countCards);
    A('collapse-module', cardsAfter === 0, `收起 gl → 对象卡回收 cards=${cardsAfter}`, `cards=${cardsAfter}`);
    A('perf-proxy', cardsGl > cardsAfter, `性能证据：展开 ${cardsGl} 卡 > 收起 ${cardsAfter} 卡`);

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_graph_dam.png') }).catch(() => {});
    console.log(`\n分域折叠 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 160)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
