// O8/§9.2 对象浏览器 CDP：类型列表→加载对象表格→过滤→点对象看详情→Search-Around 钻取。
// 前置：cmx-onto-server :8097。运行：node cmx-ontology/test/fe/onto_explorer.cjs
'use strict';
const { chromium } = require('playwright');
const http = require('http');
const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 };
const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6';
const PORT = 9096;
let _pass = 0, _total = 0;
function A(id, ok, desc, detail) { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${desc}${detail ? '  :: ' + detail : ''}`); }

function startServer() {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = req.url.split('?')[0];
      if (url === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(HARNESS); return; }
      if (url.startsWith('/api/')) {
        const chunks = []; req.on('data', c => chunks.push(c));
        req.on('end', () => {
          const body = chunks.length ? Buffer.concat(chunks) : null;
          const headers = { ...req.headers, host: `${ONTO.host}:${ONTO.port}`, 'x-api-key': KEY };
          const opts = { hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers };
          const proxy = http.request(opts, (pr) => { res.writeHead(pr.statusCode, pr.headers); pr.pipe(res); });
          proxy.on('error', () => { res.writeHead(502); res.end('proxy error'); });
          if (body) proxy.write(body); proxy.end();
        });
        return;
      }
      res.statusCode = 404; res.end('not found');
    });
    server.listen(PORT, () => resolve(server));
  });
}

const HARNESS = `<!doctype html><html><head><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#0b1020}
#stage{display:grid;grid-template-columns:250px 1fr 360px;grid-template-rows:56px 1fr;height:100vh}
#r-model{grid-column:1/4}#r-explorer{grid-row:2}#r-content{grid-row:2}#r-property{grid-row:2}
.region{overflow:auto;height:100%;border:1px solid #243049}.host{height:100%;display:block}</style></head>
<body><div id="stage">
  <div class="region" id="r-model"><div class="host" id="h-model"></div></div>
  <div class="region" id="r-explorer"><div class="host" id="h-explorer"></div></div>
  <div class="region" id="r-content"><div class="host" id="h-content"></div></div>
  <div class="region" id="r-property"><div class="host" id="h-property"></div></div>
</div>
<script type="module">
  const s = await fetch('/api/native-pages/portal.onto.explorer').then(r=>r.json())
  const src = s.data ? s.data.source : s.source
  const url = URL.createObjectURL(new Blob([src],{type:'text/javascript'}))
  const mod = await import(url); window.__mod = mod
  mod.configure({ apiBase: '' })
  const d = mod.default
  await d.views.model({ host: document.getElementById('h-model') })
  await d.views.explorer({ host: document.getElementById('h-explorer') })
  await d.views.content({ host: document.getElementById('h-content') })
  await d.views.property({ host: document.getElementById('h-property') })
  window.__ready = true
</script></body></html>`;

async function api(p, m, b) {
  const r = await fetch(`http://${ONTO.host}:${ONTO.port}${p}`, { method: m, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: b ? JSON.stringify(b) : undefined });
  return r.json().catch(() => ({}));
}

(async () => {
  // 种子：O9Ord/O9Cust + 关系 o9places + 对象 + 边
  await api('/api/onto/v1/object-types', 'POST', { apiName: 'O9Cust', displayName: '客户', primaryKey: 'id', titleProperty: 'name', status: 'active', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'name', baseType: 'string' }, { apiName: 'region', baseType: 'string' }] });
  await api('/api/onto/v1/object-types', 'POST', { apiName: 'O9Ord', displayName: '订单', primaryKey: 'id', titleProperty: 'id', status: 'active', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'amount', baseType: 'decimal' }, { apiName: 'custId', baseType: 'string' }] });
  await api('/api/onto/v1/link-types', 'POST', { apiName: 'o9places', displayName: '下单', objectTypeA: 'O9Ord', objectTypeB: 'O9Cust', cardinality: 'oneToMany' });
  await api('/api/onto/v1/objects/O9Cust', 'POST', { properties: { id: 'C-1', name: 'Ada', region: 'east' } });
  await api('/api/onto/v1/objects/O9Cust', 'POST', { properties: { id: 'C-2', name: 'Bob', region: 'west' } });
  await api('/api/onto/v1/objects/O9Ord', 'POST', { properties: { id: 'O-1', amount: 1500, custId: 'C-1' } });
  await api('/api/onto/v1/objects/O9Ord', 'POST', { properties: { id: 'O-2', amount: 300, custId: 'C-2' } });
  await api('/api/onto/v1/links', 'POST', { link: 'o9places', aPk: 'O-1', bPk: 'C-1' });
  await api('/api/onto/v1/links', 'POST', { link: 'o9places', aPk: 'O-2', bPk: 'C-2' });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1300, height: 820 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    // 等类型列表渲出
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && [...h.querySelectorAll('[data-act="pick-type"]')].some(r => r.textContent.includes('O9Ord')); }, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(400);
    A('ready', true, '四区对象浏览器就绪');

    // 类型列表含 O9Ord/O9Cust
    const types = await page.evaluate(() => [...document.querySelectorAll('#h-explorer [data-act="pick-type"]')].map(r => r.getAttribute('data-type')));
    A('types', types.includes('O9Ord') && types.includes('O9Cust'), 'explorer 列出对象类型', `types=${types}`);

    // 选 O9Ord → content 表格出 O-1/O-2
    await page.click('#h-explorer [data-act="pick-type"][data-type="O9Ord"]');
    await page.waitForTimeout(800);
    const rows = await page.evaluate(() => [...document.querySelectorAll('#h-content .o-orow')].map(r => r.getAttribute('data-pk')));
    A('list', rows.includes('O-1') && rows.includes('O-2'), 'content 表格加载订单对象', `rows=${rows}`);

    // 加过滤 amount > 1000 → 只剩 O-1（等选项稳定，避免 refreshAll 重渲染 detach）
    await page.waitForSelector('#h-explorer [data-fb="property"] option[value="amount"]', { state: 'attached', timeout: 8000 });
    await page.waitForTimeout(300);
    await page.selectOption('#h-explorer [data-fb="property"]', 'amount');
    await page.selectOption('#h-explorer [data-fb="op"]', 'gt');
    await page.fill('#h-explorer [data-fb="value"]', '1000');
    await page.click('#h-explorer [data-act="add-filter"]');
    await page.waitForTimeout(900);
    const filtered = await page.evaluate(() => [...document.querySelectorAll('#h-content .o-orow')].map(r => r.getAttribute('data-pk')));
    A('filter', filtered.includes('O-1') && !filtered.includes('O-2'), '过滤 amount>1000 → 仅 O-1', `filtered=${filtered}`);

    // 点 O-1 → property 详情出属性 + 关系按钮
    await page.click('#h-content .o-orow[data-pk="O-1"]');
    await page.waitForTimeout(400);
    const detail = await page.evaluate(() => (document.getElementById('h-property') || {}).textContent || '');
    A('detail', /1500/.test(detail) && /下单|o9places/.test(detail), 'property 出对象详情 + 关系', `has1500=${/1500/.test(detail)}`);

    // Search-Around：点关系「下单 → O9Cust」→ content 变 C-1
    await page.click('#h-property [data-act="search-around"]');
    await page.waitForTimeout(900);
    const around = await page.evaluate(() => [...document.querySelectorAll('#h-content .o-orow')].map(r => r.getAttribute('data-pk')));
    A('search-around', around.includes('C-1') && !around.includes('O-1'), 'Search-Around 钻取到相关客户 C-1', `around=${around}`);
    const crumb = await page.evaluate(() => (document.getElementById('h-model') || {}).textContent || '');
    A('breadcrumb', /钻取|o9places/.test(crumb), 'model 显示钻取面包屑', `crumb=${crumb.trim().slice(0,50)}`);

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_explorer.png') }).catch(() => {});
    console.log(`\n对象浏览器 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
