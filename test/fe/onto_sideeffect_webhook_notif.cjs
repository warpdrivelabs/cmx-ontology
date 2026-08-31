// CDP：设计工作台「动作 → Webhook / 通知副作用」可视化配置。
// 覆盖：webhook 副作用富块（URL + 请求体字段映射）、notification 富块（模板 + 通知数据字段）渲染与回填；
//       编辑 + 加字段 → 保存 → API 校验落库 sideEffects（内联键正确、无 _vars 泄漏）；新增副作用默认即富块。
// 前置：onto-server :8097。运行：NODE_PATH=/Users/nanomesh/node_modules node test/fe/onto_sideeffect_webhook_notif.cjs
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9100;
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
        }); return;
      }
      res.statusCode = 404; res.end('not found');
    });
    server.listen(PORT, () => resolve(server));
  });
}
const HARNESS = `<!doctype html><html><head><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#0b1020}
#stage{display:grid;grid-template-columns:230px 1fr 400px;grid-template-rows:64px 1fr;height:100vh}
#r-model{grid-column:1/4}#r-explorer{grid-row:2}#r-content{grid-row:2}#r-property{grid-row:2}
.region{overflow:auto;height:100%;border:1px solid #243049}.host{height:100%;display:block}</style></head>
<body><div id="stage">
  <div class="region" id="r-model"><div class="host" id="h-model"></div></div>
  <div class="region" id="r-explorer"><div class="host" id="h-explorer"></div></div>
  <div class="region" id="r-content"><div class="host" id="h-content"></div></div>
  <div class="region" id="r-property"><div class="host" id="h-property"></div></div>
</div>
<script type="module">
  const srcResp = await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json())
  const src = srcResp.data ? srcResp.data.source : srcResp.source
  const mod = await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'})))
  mod.configure({ apiBase: '' })
  const d = mod.default
  await d.views.model({ host: document.getElementById('h-model') })
  await d.views.explorer({ host: document.getElementById('h-explorer') })
  await d.views.content({ host: document.getElementById('h-content') })
  await d.views.property({ host: document.getElementById('h-property') })
  window.__ontoReady = true
</script></body></html>`;
async function api(pathname, method, body) {
  const r = await fetch(`http://${ONTO.host}:${ONTO.port}${pathname}`, { method, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: body ? JSON.stringify(body) : undefined });
  return r.json().catch(() => ({}));
}

(async () => {
  // seed：动作 wnAct，副作用 [webhook, notification]（各带一映射字段）。POST upsert，重跑归位。
  await api('/api/onto/v1/action-types', 'POST', {
    apiName: 'wnAct', displayName: '外呼与通知', status: 'active',
    parameters: [{ name: 'orderId', required: true }, { name: 'amt' }],
    logic: [], validations: [],
    sideEffects: [
      { kind: 'webhook', url: 'http://127.0.0.1:8770/hook/x', event: 'order.paid', orderId: '$orderId' },
      { kind: 'notification', template: 'orderClosed', note: '$orderId' },
    ],
  });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1340, height: 900 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ontoReady === true, { timeout: 15000 });
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && [...h.querySelectorAll('[data-sel-kind="action"]')].some(r => r.textContent.includes('wnAct')); }, { timeout: 15000 }).catch(() => {});
    A('ready', true, '四区设计台就绪');

    const opened = await page.evaluate(() => { const h = document.getElementById('h-explorer'); const row = [...h.querySelectorAll('[data-sel-kind="action"]')].find(r => r.textContent.includes('wnAct')); if (!row) return 'no-row'; row.click(); return 'ok'; });
    A('open', opened === 'ok', 'explorer 打开动作 wnAct', `ret=${opened}`);
    await page.waitForFunction(() => document.querySelectorAll('#h-property .o-fxflow').length >= 2, { timeout: 8000 }).catch(() => {});

    // 副作用块索引：0=webhook，1=notification
    const wh = await page.evaluate(() => {
      const b = document.querySelector('#h-property .o-fxblock[data-i="0"]'); if (!b) return null;
      const tag = (b.querySelector('.o-fxtag') || {}).textContent || '';
      const url = (b.querySelector('[data-sef="url"]') || {}).value;
      const rows = [...b.querySelectorAll('.o-sevar')].map(r => ({ n: (r.querySelector('[data-sev="name"]') || {}).value, v: (r.querySelector('[data-sev="value"]') || {}).value }));
      return { tag, url, rows };
    });
    A('wh-block', wh && /Webhook/.test(wh.tag) && wh.url === 'http://127.0.0.1:8770/hook/x', 'Webhook 富块渲染（URL 回填）', JSON.stringify(wh && { tag: wh.tag, url: wh.url }));
    A('wh-vars', wh && wh.rows.some(r => r.n === 'event' && r.v === 'order.paid') && wh.rows.some(r => r.n === 'orderId' && r.v === '$orderId'), 'Webhook 请求体字段派生（event/orderId）', JSON.stringify(wh && wh.rows));

    const nt = await page.evaluate(() => {
      const b = document.querySelector('#h-property .o-fxblock[data-i="1"]'); if (!b) return null;
      const tag = (b.querySelector('.o-fxtag') || {}).textContent || '';
      const tmpl = (b.querySelector('[data-sef="template"]') || {}).value;
      const rows = [...b.querySelectorAll('.o-sevar')].map(r => ({ n: (r.querySelector('[data-sev="name"]') || {}).value, v: (r.querySelector('[data-sev="value"]') || {}).value }));
      return { tag, tmpl, rows };
    });
    A('nt-block', nt && /通知/.test(nt.tag) && nt.tmpl === 'orderClosed', '通知富块渲染（模板回填）', JSON.stringify(nt && { tag: nt.tag, tmpl: nt.tmpl }));
    A('nt-var', nt && nt.rows.some(r => r.n === 'note' && r.v === '$orderId'), '通知数据字段派生（note=$orderId）', JSON.stringify(nt && nt.rows));

    // 编辑 webhook：改 URL + 加请求体字段 amount=$amt
    await page.fill('#h-property .o-fxblock[data-i="0"] [data-sef="url"]', 'http://127.0.0.1:8770/hook/paid');
    await page.click('#h-property .o-fxblock[data-i="0"] [data-act="ac-add-sevar"]');
    await page.waitForTimeout(250);
    let whRows = await page.$$('#h-property .o-fxblock[data-i="0"] .o-sevar');
    let last = whRows[whRows.length - 1];
    await (await last.$('[data-sev="name"]')).fill('amount');
    await (await last.$('[data-sev="value"]')).fill('$amt');

    // 编辑 notification：加通知数据字段 title=已关闭
    await page.click('#h-property .o-fxblock[data-i="1"] [data-act="ac-add-sevar"]');
    await page.waitForTimeout(250);
    let ntRows = await page.$$('#h-property .o-fxblock[data-i="1"] .o-sevar');
    last = ntRows[ntRows.length - 1];
    await (await last.$('[data-sev="name"]')).fill('title');
    await (await last.$('[data-sev="value"]')).fill('order-closed');
    A('edit', true, '改 URL + webhook/notification 各加一字段');

    // 保存
    await page.click('#h-property [data-act="save-action"]');
    await page.waitForTimeout(1200);

    // API 校验落库
    const def = await api('/api/onto/v1/action-types/wnAct', 'GET');
    const fx = (def.data && def.data.sideEffects) || def.sideEffects || [];
    const w = fx.find(x => x.kind === 'webhook') || {};
    const n = fx.find(x => x.kind === 'notification') || {};
    A('save-wh', w.url === 'http://127.0.0.1:8770/hook/paid' && w.event === 'order.paid' && w.orderId === '$orderId' && w.amount === '$amt', 'Webhook 落库（URL 改 + 字段 event/orderId/amount）', JSON.stringify(w));
    A('save-wh-clean', w._vars === undefined, 'Webhook 无 _vars 泄漏', `keys=${Object.keys(w)}`);
    A('save-nt', n.template === 'orderClosed' && n.note === '$orderId' && n.title === 'order-closed', '通知落库（模板 + 字段 note/title）', JSON.stringify(n));
    A('save-nt-clean', n._vars === undefined, '通知无 _vars 泄漏', `keys=${Object.keys(n)}`);

    // 新增副作用默认即富块（notification）
    await page.click('#h-property [data-act="ac-add-fx"]');
    await page.waitForTimeout(300);
    const blocks = await page.evaluate(() => document.querySelectorAll('#h-property .o-fxflow').length);
    A('add-fresh', blocks === 3, '新增副作用（默认通知）即渲染富块（共 3 块）', `flows=${blocks}`);
    // 切成 webhook → URL 输入出现
    const kinds = await page.$$('#h-property [data-sef="kind"]');
    await kinds[kinds.length - 1].selectOption('webhook');
    await page.waitForTimeout(300);
    const hasUrl = await page.evaluate(() => !!document.querySelector('#h-property .o-fxblock[data-i="2"] [data-sef="url"]'));
    A('switch-wh', hasUrl, '切「Webhook」→ URL 富块出现');

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_sideeffect_webhook_notif.png') }).catch(() => {});
    console.log(`\nWebhook/通知副作用可视化配置 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
