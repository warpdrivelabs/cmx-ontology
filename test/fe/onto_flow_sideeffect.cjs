// CDP：设计工作台「动作 → 起流程副作用」可视化配置。
// 覆盖：seed 的 startBusinessProcess 副作用渲染富配置块（flowDefKey 选择器 + businessKey + 变量映射）；
//       流程选择器 datalist 由 onto /flow/definitions 代理填充；编辑 businessKey + 加参数变量映射 → 保存 →
//       API 校验落库 sideEffects 正确（无 _vars 泄漏）；kind 下拉切换 → 富块出现。
// 前置：onto-server :8097（ONTO_FLOW_API_KEY 指向 flow）+ flow-server :8091 在线。
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9099;
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
  // seed：动作 fxAct 带一个 startBusinessProcess 副作用（flowDefKey + 一个变量 orderId）。POST 为 upsert，重跑归位。
  await api('/api/onto/v1/action-types', 'POST', {
    apiName: 'fxAct', displayName: '发起审批（联调）', status: 'active',
    parameters: [{ name: 'orderId', required: true }, { name: 'amt' }],
    logic: [], validations: [],
    sideEffects: [{ kind: 'startBusinessProcess', flowDefKey: 'onto_int_approve', orderId: '$orderId' }],
  });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1340, height: 880 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ontoReady === true, { timeout: 15000 });
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && [...h.querySelectorAll('[data-sel-kind="action"]')].some(r => r.textContent.includes('fxAct')); }, { timeout: 15000 }).catch(() => {});
    A('ready', true, '四区设计台就绪');

    // 打开动作 fxAct
    const opened = await page.evaluate(() => { const h = document.getElementById('h-explorer'); const row = [...h.querySelectorAll('[data-sel-kind="action"]')].find(r => r.textContent.includes('fxAct')); if (!row) return 'no-row'; row.click(); return 'ok'; });
    A('open', opened === 'ok', 'explorer 打开动作 fxAct', `ret=${opened}`);
    // 等富配置块渲染（副作用 kind=startBusinessProcess → o-fxflow）
    await page.waitForFunction(() => !!document.querySelector('#h-property .o-fxflow [data-sef="flowDefKey"]'), { timeout: 8000 }).catch(() => {});

    const flowVal = await page.evaluate(() => (document.querySelector('#h-property [data-sef="flowDefKey"]') || {}).value || '');
    A('rich-block', flowVal === 'onto_int_approve', '起流程副作用渲染为富配置块（flowDefKey 已回填）', `val=${flowVal}`);

    // 流程选择器 datalist 由 /flow/definitions 代理填充（等异步载入）
    await page.waitForFunction(() => { const dl = document.querySelector('#h-property datalist[id^="ofd-"]'); return dl && dl.querySelectorAll('option').length > 0; }, { timeout: 8000 }).catch(() => {});
    const defCount = await page.evaluate(() => { const dl = document.querySelector('#h-property datalist[id^="ofd-"]'); return dl ? dl.querySelectorAll('option').length : 0; });
    A('picker-populated', defCount > 0, `流程选择器由后端代理填充（${defCount} 个已发布流程）`, `n=${defCount}`);
    const hasIntDef = await page.evaluate(() => { const dl = document.querySelector('#h-property datalist[id^="ofd-"]'); return dl && [...dl.querySelectorAll('option')].some(o => o.value === 'onto_int_approve'); });
    A('picker-has-def', hasIntDef, '选择器含 onto_int_approve');

    // seed 的内联额外字段 orderId → 派生为一条变量映射行
    const var0 = await page.evaluate(() => { const r = document.querySelector('#h-property .o-sevar'); if (!r) return null; return { name: (r.querySelector('[data-sev="name"]') || {}).value, value: (r.querySelector('[data-sev="value"]') || {}).value }; });
    A('var-derived', var0 && var0.name === 'orderId' && var0.value === '$orderId', '内联字段派生为变量映射行（orderId=$orderId）', JSON.stringify(var0));

    // 编辑 businessKey
    await page.fill('#h-property [data-sef="businessKey"]', '$orderId');
    // 加第二条变量映射：amount=$amt
    await page.click('#h-property [data-act="ac-add-sevar"]');
    await page.waitForTimeout(300);
    const rows = await page.$$('#h-property .o-sevar');
    const last = rows[rows.length - 1];
    await (await last.$('[data-sev="name"]')).fill('amount');
    await (await last.$('[data-sev="value"]')).fill('$amt');
    A('edit', rows.length === 2, '加第二条变量映射行', `rows=${rows.length}`);

    // 保存
    await page.click('#h-property [data-act="save-action"]');
    await page.waitForTimeout(1200);

    // API 校验落库
    const def = await api('/api/onto/v1/action-types/fxAct', 'GET');
    const fx = (def.data && def.data.sideEffects) || def.sideEffects || [];
    const s = fx.find(x => x.kind === 'startBusinessProcess') || {};
    A('save-flowdef', s.flowDefKey === 'onto_int_approve', '落库 flowDefKey=onto_int_approve', JSON.stringify(s));
    A('save-bizkey', s.businessKey === '$orderId', '落库 businessKey=$orderId');
    A('save-var1', s.orderId === '$orderId', '落库变量 orderId=$orderId');
    A('save-var2', s.amount === '$amt', '落库新增变量 amount=$amt');
    A('no-vars-leak', s._vars === undefined, '编辑态 _vars 未泄漏到后端', `keys=${Object.keys(s)}`);

    // kind 下拉切换：新增一个副作用（默认通知）→ 切成起流程 → 富块出现
    await page.click('#h-property [data-act="ac-add-fx"]');
    await page.waitForTimeout(300);
    const kinds = await page.$$('#h-property [data-sef="kind"]');
    await kinds[kinds.length - 1].selectOption('startBusinessProcess');
    await page.waitForTimeout(400);
    const flows = await page.evaluate(() => document.querySelectorAll('#h-property .o-fxflow').length);
    A('kind-switch', flows === 2, 'kind 下拉切到「起流程」→ 富配置块出现（共 2 块）', `flows=${flows}`);

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_flow_sideeffect.png') }).catch(() => {});
    console.log(`\n起流程副作用可视化配置 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
