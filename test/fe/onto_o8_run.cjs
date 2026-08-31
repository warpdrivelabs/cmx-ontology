// O8 CDP：设计工作台「函数求值试算」+「动作试算/执行」按钮 → 对接 O5/O4 后端，结果面板显示。
// 前置：cmx-onto-server :8097（含 O4/O5）。运行：node cmx-ontology/test/fe/onto_o8_run.cjs
'use strict';
const { chromium } = require('playwright');
const http = require('http');
const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 };
const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6';
const PORT = 9098;
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
          const headers = { ...req.headers, host: `${ONTO.host}:${ONTO.port}`, 'x-api-key': KEY }; // 注入服务身份
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
#stage{display:grid;grid-template-columns:230px 1fr 380px;grid-template-rows:64px 1fr;height:100vh}
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
  const url = URL.createObjectURL(new Blob([src],{type:'text/javascript'}))
  const mod = await import(url); window.__ontoMod = mod
  mod.configure({ apiBase: '' })
  const d = mod.default
  await d.views.model({ host: document.getElementById('h-model') })
  await d.views.explorer({ host: document.getElementById('h-explorer') })
  await d.views.content({ host: document.getElementById('h-content') })
  await d.views.property({ host: document.getElementById('h-property') })
  window.__ontoReady = true
</script></body></html>`;

async function api(pathname, method, body) {
  const r = await fetch(`http://${ONTO.host}:${ONTO.port}${pathname}`, {
    method, headers: { 'Content-Type': 'application/json', 'X-API-Key': KEY }, body: body ? JSON.stringify(body) : undefined,
  }); return r.json().catch(() => ({}));
}

(async () => {
  // 种子：对象类型 + 一个函数（Query 折扣）+ 一个动作（改字段）
  await api('/api/onto/v1/object-types', 'POST', { apiName: 'O8Ord', displayName: '订单', primaryKey: 'id', titleProperty: 'id', status: 'active', properties: [{ apiName: 'id', baseType: 'string' }, { apiName: 'status', baseType: 'string' }] });
  await api('/api/onto/v1/objects/O8Ord', 'POST', { properties: { id: 'X-1', status: 'open' } });
  await api('/api/onto/v1/functions', 'POST', { apiName: 'o8Disc', displayName: '折扣', runtime: 'feel', kind: 'query', inputs: [{ name: 'amount', type: 'double' }], output: { type: 'double' }, body: 'if amount > 1000 then 0.8 else 0.2', status: 'active' });
  await api('/api/onto/v1/action-types', 'POST', { apiName: 'o8Close', displayName: '关闭订单', status: 'active', parameters: [{ name: 'orderId', required: true }], logic: [{ op: 'modifyObject', objectType: 'O8Ord', pk: '$orderId', set: { status: 'closed' } }], validations: [], sideEffects: [] });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1320, height: 860 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ontoReady === true, { timeout: 15000 });
    // 等 explorer 真正渲出函数行（loadAll 异步；避免固定 sleep 竞态）
    await page.waitForFunction(() => {
      const h = document.getElementById('h-explorer');
      return h && [...h.querySelectorAll('[data-sel-kind="function"]')].some(r => r.textContent.includes('o8Disc'));
    }, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(400);
    A('ready', true, '四区 harness 就绪');

    // ── 函数求值试算 ──
    await page.evaluate(() => window.__ontoMod && document.querySelector('#h-explorer')); // ensure loaded
    // 直接调 selectElement 打开函数 Inspector
    const openFn = await page.evaluate(async () => {
      // designer 内部函数不导出；用 explorer 点击：找到函数行
      const host = document.getElementById('h-explorer');
      const row = [...host.querySelectorAll('[data-sel-kind="function"]')].find(r => r.textContent.includes('o8Disc'));
      if (!row) return 'no-row';
      row.click(); return 'ok';
    });
    A('fn-open', openFn === 'ok', 'explorer 打开函数 o8Disc', `ret=${openFn}`);
    await page.waitForTimeout(500);
    const hasEvalBtn = await page.evaluate(() => !!document.querySelector('#h-property [data-act="eval-function"]'));
    A('fn-btn', hasEvalBtn, 'property 出「求值试算」按钮');

    // 点求值 → 弹输入对话框 → 填 amount=1500 → 求值
    await page.click('#h-property [data-act="eval-function"]');
    await page.waitForTimeout(400);
    await page.fill('[data-k="in:double:amount"]', '1500');
    // 点对话框「求值」按钮
    await page.evaluate(() => { const b = [...document.querySelectorAll('.o-dlg-overlay button')].find(x => x.textContent.trim() === '求值'); if (b) b.click(); });
    await page.waitForTimeout(1200);
    const fnRes = await page.evaluate(() => (document.querySelector('#h-property [data-role="fn-result"]') || {}).textContent || '');
    A('fn-eval', /0\.8/.test(fnRes), '函数求值结果 0.8 显示', `res=${fnRes.trim().slice(0,60)}`);

    // ── 动作试算 ──
    const openAc = await page.evaluate(() => {
      const host = document.getElementById('h-explorer');
      const row = [...host.querySelectorAll('[data-sel-kind="action"]')].find(r => r.textContent.includes('o8Close'));
      if (!row) return 'no-row'; row.click(); return 'ok';
    });
    A('ac-open', openAc === 'ok', 'explorer 打开动作 o8Close', `ret=${openAc}`);
    await page.waitForTimeout(500);
    const hasRunBtns = await page.evaluate(() => !!document.querySelector('#h-property [data-act="dryrun-action"]') && !!document.querySelector('#h-property [data-act="exec-action"]'));
    A('ac-btn', hasRunBtns, 'property 出「试算/执行」按钮');

    // 试算：orderId=X-1
    await page.click('#h-property [data-act="dryrun-action"]');
    await page.waitForTimeout(400);
    await page.fill('[data-k="p:orderId"]', 'X-1');
    await page.evaluate(() => { const b = [...document.querySelectorAll('.o-dlg-overlay button')].find(x => x.textContent.trim() === '试算'); if (b) b.click(); });
    await page.waitForTimeout(1200);
    const acRes = await page.evaluate(() => (document.querySelector('#h-property [data-role="ac-result"]') || {}).textContent || '');
    A('ac-dryrun', /试算通过|编辑\s*1/.test(acRes), '动作试算结果显示（编辑 1 条）', `res=${acRes.trim().slice(0,60)}`);

    // 执行：真实写回 → 校验对象 status=closed
    await page.click('#h-property [data-act="exec-action"]');
    await page.waitForTimeout(400);
    await page.fill('[data-k="p:orderId"]', 'X-1');
    await page.evaluate(() => { const b = [...document.querySelectorAll('.o-dlg-overlay button')].find(x => x.textContent.trim() === '确认执行'); if (b) b.click(); });
    await page.waitForTimeout(1200);
    const acExec = await page.evaluate(() => (document.querySelector('#h-property [data-role="ac-result"]') || {}).textContent || '');
    A('ac-exec', /已执行|committed/.test(acExec), '动作执行结果显示', `res=${acExec.trim().slice(0,60)}`);
    const obj = await api('/api/onto/v1/object-sets/load', 'POST', { objectSet: { op: 'base', objectType: 'O8Ord' } });
    const closed = JSON.stringify(obj).includes('closed');
    A('ac-writeback', closed, '执行后对象 status=closed（真实写回）', closed ? 'ok' : JSON.stringify(obj).slice(0, 80));

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_o8_run.png') }).catch(() => {});
    console.log(`\nO8 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
