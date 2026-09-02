// CDP：业务单据 = 单一对象（多层次内容），卡片体现层级。
// 覆盖：① DOC 导入 → 只产 1 个对象类型(非头/行两卡)、0 组合关系；② 本体图卡片内出「行」层块(缩进+层名+array 徽标+层内字段);
//       ③ 层块字段(qty)可见;④ 后端 constraints.children 承载行字段。
// 前置：cmx-onto-server :8097。运行：NODE_PATH=/Users/nanomesh/node_modules node test/fe/onto_doc_single_object.cjs
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9108;
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
  globalThis.__cmxDataComp = { apiJson: async (url, options, CFG) => { const full = (CFG && CFG.apiBase && url[0] === '/') ? CFG.apiBase + url : url; const r = await fetch(full, { ...((CFG && CFG.fetchInit) || {}), ...(options || {}), headers: { Accept: 'application/json', ...((CFG && CFG.authHeaders && CFG.authHeaders()) || {}), ...((options && options.headers) || {}) } }); let j = null; try { j = await r.json(); } catch {} if (!r.ok || (j && typeof j.code === 'number' && j.code !== 0)) throw new Error((j && (j.msg || j.error)) || ('HTTP ' + r.status)); return j && typeof j === 'object' && 'data' in j ? j.data : j; }, escHtml: (s) => String(s == null ? '' : s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])) };
  const s = await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json()); const src = s.data ? s.data.source : s.source;
  const mod = await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'}))); mod.configure({ apiBase: '' }); const d = mod.default;
  await d.views.model({host:document.getElementById('h-model')}); await d.views.explorer({host:document.getElementById('h-explorer')});
  await d.views.content({host:document.getElementById('h-content')}); await d.views.property({host:document.getElementById('h-property')});
  window.__ready = true;
</script></body></html>`;
async function api(p, m, b) { const r = await fetch(`http://${ONTO.host}:${ONTO.port}${p}`, { method: m, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: b ? JSON.stringify(b) : undefined }); return r.json().catch(() => ({})); }
const GS = `function gs(){const st=[document];while(st.length){const r=st.pop();const el=r.querySelector&&r.querySelector('cmx-ontology-graph');if(el&&el.shadowRoot)return el.shadowRoot;const all=r.querySelectorAll?r.querySelectorAll('*'):[];for(const e of all){if(e.shadowRoot)st.push(e.shadowRoot)}}return null}`;
const clickKey = k => `(()=>{${GS};const s=gs();const g=s&&s.querySelector('[data-group-toggle="${k}"]');if(g){g.dispatchEvent(new MouseEvent('click',{bubbles:true}));return true}return false})()`;

(async () => {
  // 种一张单据 DocOneOrder（头 → 行 → 明细 三层），DAM=fi/cmxfico/dsd（独立模块避免和别的测试混）。
  const r = await api('/api/onto/v1/import/doc', 'POST', {
    apiName: 'DocOneOrder', displayName: '单据整体订单', dam: { domain: 'fi', application: 'cmxfico', module: 'dsd' },
    entities: [
      { apiName: 'DooHead', displayName: '订单头', primaryKey: 'id', titleProperty: 'orderNo', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'orderNo', baseType: 'string' }, { apiName: 'amount', baseType: 'decimal' }] },
      { apiName: 'DooLine', displayName: '订单行', primaryKey: 'lineId', properties: [{ apiName: 'lineId', baseType: 'string' }, { apiName: 'qty', baseType: 'long' }] },
      { apiName: 'DooDetail', displayName: '行明细', primaryKey: 'did', properties: [{ apiName: 'did', baseType: 'string' }, { apiName: 'batch', baseType: 'string' }] },
    ],
    relations: [{ from: 'DooHead', to: 'DooLine', role: 'lines', cardinality: 'oneToMany' }, { from: 'DooLine', to: 'DooDetail', role: 'details', cardinality: 'oneToMany' }],
  });

  // ① 后端：只产 1 对象、0 关系
  A('import-one-object', Array.isArray(r.data && r.data.objectTypes) && r.data.objectTypes.length === 1 && r.data.objectTypes[0] === 'DooHead', `DOC → 单一对象(DooHead)`, JSON.stringify(r.data));
  A('import-no-links', (r.data && r.data.createdLinks) === 0, 'DOC → 0 组合关系(单据是整体)');
  // ② 后端 constraints.children 承载行字段 + 层中层
  const def = await api('/api/onto/v1/object-types/DooHead', 'GET');
  const d = def.data || {};
  const lines = (d.properties || []).find(p => p.apiName === 'lines');
  const linesOk = lines && lines.baseType === 'array' && (lines.constraints || {}).level === true;
  const kids = lines && (lines.constraints || {}).children || [];
  const hasQty = kids.some(k => k.apiName === 'qty');
  const deep = kids.find(k => k.apiName === 'details');
  A('level-array', linesOk, 'lines 层块 = array + constraints.level', `bt=${lines && lines.baseType}`);
  A('level-children', hasQty, '行层 constraints.children 含字段 qty');
  A('level-nested', !!(deep && (deep.constraints || {}).entity === 'DooDetail'), '行层内再嵌明细层(层中层)');

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 950 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [err]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    await page.waitForFunction(`(()=>{${GS};const s=gs();return !!(s&&(s.querySelector('.og-grp')||s.querySelector('.og-object')))})()`, { timeout: 15000 }).catch(() => {});
    // 展开到 dsd 模块让 DooHead 卡出现
    for (const k of ['fi', 'fi/cmxfico', 'fi/cmxfico/dsd']) { await page.evaluate(clickKey(k)); await page.waitForTimeout(300); }
    await page.waitForTimeout(400);
    A('ready', true, '设计台 + 本体图就绪');

    // ③ 卡片：DooHead 单卡内出层块（非两卡）
    const info = await page.evaluate(`(()=>{${GS};const s=gs();return {
      dooCard: !!s.querySelector('[data-node="DooHead"]'),
      dooLineCard: !!s.querySelector('[data-node="DooLine"]'),
      bands: s.querySelectorAll('.og-level-band').length,
      hds: [...s.querySelectorAll('.og-level-hd')].map(t=>t.textContent),
      text: (s.querySelector('[data-node="DooHead"]')||{}).textContent||''
    }})()`);
    A('single-card', info.dooCard && !info.dooLineCard, 'DooHead 单卡出现、DooLine 无独立卡(单据整体)', `line-card=${info.dooLineCard}`);
    A('level-band', info.bands >= 1, `卡内出层块背景带(${info.bands})`);
    A('level-header', info.hds.some(h => /lines|订单行|DooLine/.test(h) && /array/.test(h)), '层头出「行」层名 + array 徽标', `hds=${JSON.stringify(info.hds)}`);
    A('level-field-in-card', /qty/.test(info.text), '卡内层块含行字段 qty');
    A('level-nested-header', info.hds.some(h => /details|明细|DooDetail/.test(h)), '卡内出层中层「明细」层头');

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_doc_single_object.png') }).catch(() => {});
    console.log(`\n业务单据单对象 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass === _total ? 0 : 1); }
})();
