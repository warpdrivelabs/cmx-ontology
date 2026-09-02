// CDP：对象浏览器/设计台 按 DAM 分组 + 业务单据类型分组。
// 覆盖：① 设计台 explorer「对象类型」按 DAM 域▸应用▸模块 折叠树；② 对象浏览器 域▸应用▸模块▸单据类型▸对象类型 五级树，
//       钻到单据类型见头/行两个对象类型，点对象类型 content 出实例行；③ 折叠切换。
// 前置：cmx-onto-server :8097（含 dam/docType 富化清单）。运行：NODE_PATH=/Users/nanomesh/node_modules node test/fe/onto_explorer_grouping.cjs
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9107;
let _pass = 0, _total = 0;
function A(id, ok, d, x) { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${d}${x ? '  :: ' + x : ''}`); }

function startServer(page) {
  return new Promise((res) => {
    const s = http.createServer((req, rq) => {
      const u = req.url.split('?')[0];
      if (u === '/explorer' || u === '/designer') { rq.setHeader('Content-Type', 'text/html; charset=utf-8'); rq.end(HARNESS(u === '/designer' ? 'portal.onto.designer' : 'portal.onto.explorer')); return; }
      if (u.startsWith('/api/')) { const c = []; req.on('data', x => c.push(x)); req.on('end', () => { const b = c.length ? Buffer.concat(c) : null; const o = { hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}`, 'x-api-key': KEY } }; const p = http.request(o, pr => { rq.writeHead(pr.statusCode, pr.headers); pr.pipe(rq); }); p.on('error', () => { rq.writeHead(502); rq.end(); }); if (b) p.write(b); p.end(); }); return; }
      rq.statusCode = 404; rq.end();
    });
    s.listen(PORT, () => res(s));
  });
}
const HARNESS = (pageId) => `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;height:100%;background:#0b1020}#stage{display:grid;grid-template-columns:280px 1fr 360px;grid-template-rows:56px 1fr;height:100vh}#r-model{grid-column:1/4}.region{overflow:auto;height:100%;border:1px solid #243049}.host{height:100%;display:block}</style></head>
<body><div id="stage"><div class="region" id="r-model"><div class="host" id="h-model"></div></div><div class="region" id="r-explorer"><div class="host" id="h-explorer"></div></div><div class="region" id="r-content"><div class="host" id="h-content"></div></div><div class="region" id="r-property"><div class="host" id="h-property"></div></div></div>
<script type="module">
  globalThis.__cmxDataComp = { apiJson: async (url, options, CFG) => { const full = (CFG && CFG.apiBase && url[0] === '/') ? CFG.apiBase + url : url; const r = await fetch(full, { ...((CFG && CFG.fetchInit) || {}), ...(options || {}), headers: { Accept: 'application/json', ...((CFG && CFG.authHeaders && CFG.authHeaders()) || {}), ...((options && options.headers) || {}) } }); let j = null; try { j = await r.json(); } catch {} if (!r.ok || (j && typeof j.code === 'number' && j.code !== 0)) throw new Error((j && (j.msg || j.error)) || ('HTTP ' + r.status)); return j && typeof j === 'object' && 'data' in j ? j.data : j; }, escHtml: (s) => String(s == null ? '' : s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])) };
  const s = await fetch('/api/native-pages/${pageId}').then(r=>r.json()); const src = s.data ? s.data.source : s.source;
  const mod = await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'}))); mod.configure({ apiBase: '' }); const d = mod.default;
  await d.views.model({host:document.getElementById('h-model')}); await d.views.explorer({host:document.getElementById('h-explorer')});
  await d.views.content({host:document.getElementById('h-content')}); await d.views.property({host:document.getElementById('h-property')});
  window.__ready = true;
</script></body></html>`;
async function api(p, m, b) { const r = await fetch(`http://${ONTO.host}:${ONTO.port}${p}`, { method: m, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: b ? JSON.stringify(b) : undefined }); return r.json().catch(() => ({})); }

(async () => {
  // 种子：一张业务单据 GrpSalesOrder（fi/cmxfico/sd）→ **单一对象 GrpSoHead**（行作为嵌套层，非独立对象）；种几条实例。
  await api('/api/onto/v1/import/doc', 'POST', {
    apiName: 'GrpSalesOrder', displayName: '分组销售订单', dam: { domain: 'fi', application: 'cmxfico', module: 'sd' },
    entities: [
      { apiName: 'GrpSoHead', displayName: '订单头', primaryKey: 'id', titleProperty: 'id', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'amount', baseType: 'decimal' }] },
      { apiName: 'GrpSoLine', displayName: '订单行', primaryKey: 'lid', titleProperty: 'lid', properties: [{ apiName: 'lid', baseType: 'string' }] },
    ],
    relations: [{ from: 'GrpSoHead', to: 'GrpSoLine', role: 'lines' }],
  });
  await api('/api/onto/v1/objects/GrpSoHead', 'POST', { properties: { id: 'GH-1', amount: 900 } });
  await api('/api/onto/v1/objects/GrpSoHead', 'POST', { properties: { id: 'GH-2', amount: 100 } });

  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const server = await startServer(page);
  page.on('console', m => { if (m.type() === 'error') console.log('  [err]', m.text()); });
  try {
    // ── A) 设计台 explorer：对象类型按 DAM 折叠树 ──
    await page.goto(`http://127.0.0.1:${PORT}/designer`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && h.querySelector('[data-act="dam-toggle"]'); }, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(400);
    A('d-ready', true, '设计台四区就绪');
    const damNodes = await page.evaluate(() => [...document.querySelectorAll('#h-explorer [data-act="dam-toggle"]')].map(n => n.getAttribute('data-key')));
    A('d-dam-tree', damNodes.some(k => k === 'fi') && damNodes.some(k => k === 'fi/cmxfico') && damNodes.some(k => k === 'fi/cmxfico/sd'), '对象类型出现 DAM 三级折叠节点(fi▸cmxfico▸sd)', `keys=${damNodes.join(',')}`);
    // 叶子对象类型在展开的 sd 下可见
    const dLeaf = await page.evaluate(() => !!document.querySelector('#h-explorer [data-sel-id="GrpSoHead"]'));
    A('d-leaf', dLeaf, '模块 sd 下见对象类型叶子 GrpSoHead(默认展开)');
    // 收起 fi → 叶子消失
    await page.click('#h-explorer [data-act="dam-toggle"][data-key="fi"]'); await page.waitForTimeout(250);
    const dCollapsed = await page.evaluate(() => !document.querySelector('#h-explorer [data-sel-id="GrpSoHead"]'));
    A('d-collapse', dCollapsed, '收起 fi 域 → 对象类型叶子回收');
    // 选中叶子 → property Inspector 出「业务单据类型」输入
    await page.click('#h-explorer [data-act="dam-toggle"][data-key="fi"]'); await page.waitForTimeout(200);
    await page.click('#h-explorer [data-sel-id="GrpSoHead"]'); await page.waitForTimeout(500);
    const inspHasDoc = await page.evaluate(() => { const h = document.getElementById('h-property'); return !!(h && h.querySelector('[data-df="docType.code"]')); });
    const docVal = await page.evaluate(() => { const el = document.querySelector('#h-property [data-df="docType.code"]'); return el ? el.value : ''; });
    A('d-inspector-doc', inspHasDoc && docVal === 'GrpSalesOrder', 'Inspector 出业务单据类型输入且回填 GrpSalesOrder', `val=${docVal}`);

    // ── B) 对象浏览器：五级树 域▸应用▸模块▸单据类型▸对象类型 ──
    await page.goto(`http://127.0.0.1:${PORT}/explorer`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && h.querySelector('[data-act="tree-toggle"]'); }, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(400);
    A('e-ready', true, '对象浏览器就绪');
    const treeKeys = await page.evaluate(() => [...document.querySelectorAll('#h-explorer [data-act="tree-toggle"]')].map(n => n.getAttribute('data-key')));
    const hasFive = ['fi', 'fi/cmxfico', 'fi/cmxfico/sd', 'fi/cmxfico/sd/分组销售订单'].every(k => treeKeys.includes(k));
    A('e-5level', hasFive, '五级树 域▸应用▸模块▸单据类型 全部出现', `keys=${treeKeys.join(' | ')}`);
    // 单据类型节点下见对象类型（业务单据=单一对象 GrpSoHead，行为其嵌套层）
    const docHasType = await page.evaluate(() => !!document.querySelector('#h-explorer [data-type="GrpSoHead"]'));
    const lineNotType = await page.evaluate(() => !document.querySelector('#h-explorer [data-type="GrpSoLine"]'));
    A('e-doc-groups-types', docHasType && lineNotType, '业务单据类型下归到单一对象类型(GrpSoHead)、行非独立对象');
    // 点对象类型叶子 GrpSoHead → content 出实例行 GH-1/GH-2
    await page.click('#h-explorer [data-type="GrpSoHead"]'); await page.waitForTimeout(800);
    const rows = await page.evaluate(() => [...document.querySelectorAll('#h-content .o-orow')].map(r => r.getAttribute('data-pk')));
    A('e-instances', rows.includes('GH-1') && rows.includes('GH-2'), '点对象类型 → content 出实例行(GH-1/GH-2)', `rows=${rows}`);
    // 收起单据类型 → 对象类型叶子消失
    await page.click('#h-explorer [data-act="tree-toggle"][data-key="fi/cmxfico/sd/分组销售订单"]'); await page.waitForTimeout(250);
    const eCollapsed = await page.evaluate(() => !document.querySelector('#h-explorer [data-type="GrpSoHead"]'));
    A('e-collapse', eCollapsed, '收起单据类型 → 对象类型叶子回收');

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_explorer_grouping.png') }).catch(() => {});
    console.log(`\n分组 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass === _total ? 0 : 1); }
})();
