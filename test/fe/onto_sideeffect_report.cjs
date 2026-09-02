// CDP：设计工作台「动作 → 生成报表(computeReport)副作用」可视化配置。
// 覆盖：computeReport 富块（reportCode 选择器 + 报表参数映射）渲染/回填；选择器由 onto /report/definitions
//       代理填充（cmx-report 报表）；编辑加参数 → 保存 → API 校验落库 sideEffects（无 _vars 泄漏）；kind 切换出富块。
// 前置：onto-server :8097（ONTO_REPORT_API_KEY 指向 report）+ report-server :8092 在线。
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9101;
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
  // seed：动作 rptAct 带 computeReport 副作用（reportCode + orgCode/periodCode 映射）。POST upsert。
  await api('/api/onto/v1/action-types', 'POST', {
    apiName: 'rptAct', displayName: '生成报表（联调）', status: 'active',
    parameters: [{ name: 'org', required: true }, { name: 'period', required: true }],
    logic: [], validations: [],
    sideEffects: [{ kind: 'computeReport', reportCode: 'STAT_01_D', orgCode: '$org', periodCode: '$period' }],
  });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1340, height: 900 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ontoReady === true, { timeout: 15000 });
    await page.waitForFunction(() => { const h = document.getElementById('h-explorer'); return h && [...h.querySelectorAll('[data-sel-kind="action"]')].some(r => r.textContent.includes('rptAct')); }, { timeout: 15000 }).catch(() => {});
    A('ready', true, '四区设计台就绪');

    const opened = await page.evaluate(() => { const h = document.getElementById('h-explorer'); const row = [...h.querySelectorAll('[data-sel-kind="action"]')].find(r => r.textContent.includes('rptAct')); if (!row) return 'no-row'; row.click(); return 'ok'; });
    A('open', opened === 'ok', 'explorer 打开动作 rptAct', `ret=${opened}`);
    await page.waitForFunction(() => !!document.querySelector('#h-property .o-fxflow [data-sef="reportCode"]'), { timeout: 8000 }).catch(() => {});

    const rc = await page.evaluate(() => (document.querySelector('#h-property [data-sef="reportCode"]') || {}).value || '');
    A('rich-block', rc === 'STAT_01_D', '生成报表副作用富块（reportCode 回填）', `val=${rc}`);

    // 报表选择器 datalist 由 /report/definitions 代理填充
    await page.waitForFunction(() => { const dl = document.querySelector('#h-property datalist[id^="orp-"]'); return dl && dl.querySelectorAll('option').length > 0; }, { timeout: 8000 }).catch(() => {});
    const repCount = await page.evaluate(() => { const dl = document.querySelector('#h-property datalist[id^="orp-"]'); return dl ? dl.querySelectorAll('option').length : 0; });
    A('picker-populated', repCount > 0, `报表选择器由后端代理填充（${repCount} 张报表）`, `n=${repCount}`);
    const hasRpt = await page.evaluate(() => { const dl = document.querySelector('#h-property datalist[id^="orp-"]'); return dl && [...dl.querySelectorAll('option')].some(o => o.value === 'STAT_01_D'); });
    A('picker-has', hasRpt, '选择器含 STAT_01_D');

    // orgCode/periodCode 派生为参数行
    const rows0 = await page.evaluate(() => [...document.querySelectorAll('#h-property .o-sevar')].map(r => ({ n: (r.querySelector('[data-sev="name"]') || {}).value, v: (r.querySelector('[data-sev="value"]') || {}).value })));
    A('vars-derived', rows0.some(r => r.n === 'orgCode' && r.v === '$org') && rows0.some(r => r.n === 'periodCode' && r.v === '$period'), '报表参数派生（orgCode/periodCode）', JSON.stringify(rows0));

    // 加参数 version=V2
    await page.click('#h-property [data-act="ac-add-sevar"]');
    await page.waitForTimeout(250);
    const rows = await page.$$('#h-property .o-sevar');
    const last = rows[rows.length - 1];
    await (await last.$('[data-sev="name"]')).fill('version');
    await (await last.$('[data-sev="value"]')).fill('V2');
    A('edit', rows.length === 3, '加第三条参数（version）', `rows=${rows.length}`);

    await page.click('#h-property [data-act="save-action"]');
    await page.waitForTimeout(1200);

    const def = await api('/api/onto/v1/action-types/rptAct', 'GET');
    const fx = (def.data && def.data.sideEffects) || def.sideEffects || [];
    const s = fx.find(x => x.kind === 'computeReport') || {};
    A('save-report', s.reportCode === 'STAT_01_D', '落库 reportCode=STAT_01_D', JSON.stringify(s));
    A('save-org', s.orgCode === '$org' && s.periodCode === '$period', '落库参数 orgCode/periodCode');
    A('save-version', s.version === 'V2', '落库新增参数 version=V2');
    A('no-vars-leak', s._vars === undefined, '编辑态 _vars 未泄漏', `keys=${Object.keys(s)}`);

    // kind 切换：新增副作用 → 切成 computeReport → 富块出现
    await page.click('#h-property [data-act="ac-add-fx"]');
    await page.waitForTimeout(300);
    const kinds = await page.$$('#h-property [data-sef="kind"]');
    await kinds[kinds.length - 1].selectOption('computeReport');
    await page.waitForTimeout(400);
    const hasRc2 = await page.evaluate(() => document.querySelectorAll('#h-property [data-sef="reportCode"]').length >= 2);
    A('kind-switch', hasRc2, 'kind 切「生成报表」→ reportCode 富块出现');

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_sideeffect_report.png') }).catch(() => {});
    console.log(`\n生成报表副作用可视化配置 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
