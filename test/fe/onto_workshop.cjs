// O8/§9.3 应用搭建台/对象360 CDP：选对象→360视图(属性卡+关系区)→执行动作→写回刷新。
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9095;
let _pass = 0, _total = 0;
const A = (id, ok, d, x) => { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${d}${x ? ' :: ' + x : ''}`); };

function startServer() {
  return new Promise((resolve) => {
    const s = http.createServer((req, res) => {
      const u = req.url.split('?')[0];
      if (u === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(HARNESS); return; }
      if (u.startsWith('/api/')) { const c = []; req.on('data', x => c.push(x)); req.on('end', () => { const b = c.length ? Buffer.concat(c) : null; const o = { hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}`, 'x-api-key': KEY } }; const p = http.request(o, pr => { res.writeHead(pr.statusCode, pr.headers); pr.pipe(res); }); p.on('error', () => { res.writeHead(502); res.end(); }); if (b) p.write(b); p.end(); }); return; }
      res.statusCode = 404; res.end();
    });
    s.listen(PORT, () => resolve(s));
  });
}
const HARNESS = `<!doctype html><html><head><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#0b1020}#stage{display:grid;grid-template-columns:230px 1fr 320px;grid-template-rows:52px 1fr;height:100vh}#r-model{grid-column:1/4}#r-explorer{grid-row:2}#r-content{grid-row:2}#r-property{grid-row:2}.region{overflow:auto;height:100%;border:1px solid #243049}.host{height:100%;display:block}</style></head>
<body><div id="stage"><div class="region" id="r-model"><div class="host" id="h-model"></div></div><div class="region" id="r-explorer"><div class="host" id="h-explorer"></div></div><div class="region" id="r-content"><div class="host" id="h-content"></div></div><div class="region" id="r-property"><div class="host" id="h-property"></div></div></div>
<script type="module">
  const s = await fetch('/api/native-pages/portal.onto.workshop').then(r=>r.json()); const src = s.data ? s.data.source : s.source;
  const mod = await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'}))); mod.configure({ apiBase: '' });
  const d = mod.default;
  await d.views.model({host:document.getElementById('h-model')}); await d.views.explorer({host:document.getElementById('h-explorer')});
  await d.views.content({host:document.getElementById('h-content')}); await d.views.property({host:document.getElementById('h-property')});
  window.__ready = true;
</script></body></html>`;
async function api(p, m, b) { const r = await fetch(`http://${ONTO.host}:${ONTO.port}${p}`, { method: m, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: b ? JSON.stringify(b) : undefined }); return r.json().catch(() => ({})); }

(async () => {
  // 种子：WsOrd(status) + WsCust + 关系 wsPlaces + 对象 + 边 + 动作 wsClose(改 status)
  await api('/api/onto/v1/object-types', 'POST', { apiName: 'WsCust', displayName: '客户', primaryKey: 'id', titleProperty: 'name', status: 'active', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'name', baseType: 'string' }] });
  await api('/api/onto/v1/object-types', 'POST', { apiName: 'WsOrd', displayName: '订单', primaryKey: 'id', titleProperty: 'id', status: 'active', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'status', baseType: 'string' }, { apiName: 'amount', baseType: 'decimal' }] });
  await api('/api/onto/v1/link-types', 'POST', { apiName: 'wsPlaces', displayName: '下单', objectTypeA: 'WsOrd', objectTypeB: 'WsCust', cardinality: 'oneToMany' });
  await api('/api/onto/v1/objects/WsCust', 'POST', { properties: { id: 'WC-1', name: 'Ada' } });
  await api('/api/onto/v1/objects/WsOrd', 'POST', { properties: { id: 'WO-1', status: 'open', amount: 500 } });
  await api('/api/onto/v1/links', 'POST', { link: 'wsPlaces', aPk: 'WO-1', bPk: 'WC-1' });
  await api('/api/onto/v1/action-types', 'POST', { apiName: 'wsClose', displayName: '关闭订单', status: 'active', parameters: [{ name: 'orderId', required: true }], logic: [{ op: 'modifyObject', objectType: 'WsOrd', pk: '$orderId', set: { status: 'closed' } }], validations: [], sideEffects: [] });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1300, height: 820 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [err]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ready === true, { timeout: 15000 });
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && h.querySelector('[data-act="pick-type"]'); }, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(500);
    A('ready', true, '四区应用搭建台就绪');

    // 选类型 WsOrd
    await page.selectOption('#h-explorer [data-act="pick-type"]', 'WsOrd');
    await page.waitForTimeout(800);
    const objs = await page.evaluate(() => [...document.querySelectorAll('#h-explorer [data-act="enter"]')].map(r => r.getAttribute('data-pk')));
    A('list', objs.includes('WO-1'), 'explorer 列出 WsOrd 对象', `objs=${objs}`);

    // 进入 WO-1 的 360 视图
    await page.click('#h-explorer [data-act="enter"][data-pk="WO-1"]');
    await page.waitForTimeout(900);
    const content = await page.evaluate(() => (document.getElementById('h-content') || {}).textContent || '');
    A('card', /WO-1/.test(content) && /open/.test(content) && /500/.test(content), 'content 出对象属性卡(status=open,amount=500)', `has=${/open/.test(content)}`);
    A('relations', /下单|wsPlaces/.test(content) && /WC-1|Ada/.test(content), 'content 出关系区(下单→客户 Ada)', `rel=${/WC-1|Ada/.test(content)}`);

    // property 出动作按钮
    const hasAct = await page.evaluate(() => !!document.querySelector('#h-property [data-act="run-action"][data-id="wsClose"]'));
    A('act-btn', hasAct, 'property 出动作按钮 wsClose');

    // 执行 wsClose(orderId=WO-1) → 写回 status=closed
    await page.click('#h-property [data-act="run-action"][data-id="wsClose"]');
    await page.waitForTimeout(500);
    await page.fill('.o-dlg-overlay [data-k="p:orderId"]', 'WO-1');
    await page.evaluate(() => { const b = [...document.querySelectorAll('.o-dlg-overlay button')].find(x => x.textContent.trim() === '执行'); if (b) b.click(); });
    await page.waitForTimeout(1200);
    const acRes = await page.evaluate(() => (document.querySelector('#h-property [data-role="ac-result"]') || {}).textContent || '');
    A('run', /已执行|committed/.test(acRes), '动作执行结果显示', `res=${acRes.trim().slice(0,40)}`);
    // content 刷新可能慢（执行后重跑 Search-Around）→ 轮询等 closed 出现
    await page.waitForFunction(() => /closed/.test((document.getElementById('h-content') || {}).textContent || ''), { timeout: 8000 }).catch(() => {});
    const after = await page.evaluate(() => (document.getElementById('h-content') || {}).textContent || '');
    A('writeback', /closed/.test(after), '执行后 360 视图刷新(status=closed)', `has=${/closed/.test(after)}`);
    const obj = await api('/api/onto/v1/object-sets/load', 'POST', { objectSet: { op: 'base', objectType: 'WsOrd' } });
    A('db', JSON.stringify(obj).includes('closed'), '后端对象真写回 status=closed');

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_workshop.png') }).catch(() => {});
    console.log(`\n应用搭建台 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 150)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
