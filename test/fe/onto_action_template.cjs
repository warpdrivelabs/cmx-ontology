// CDP：设计工作台「从模板新建动作」——选内置模板 consolClose（关账联动）→ 建动作 → 编辑器自动呈现
//       两条副作用富块（起流程 consol_close + 生成报表 STAT_01_D）。前置：onto :8097 + flow :8091 + report :8092。
'use strict';
const { chromium } = require('playwright');
const http = require('http'); const path = require('path');
const ONTO = { host: '127.0.0.1', port: 8097 }; const KEY = 'cmx_sk_dev_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6'; const PORT = 9102;
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
#stage{display:grid;grid-template-columns:240px 1fr 400px;grid-template-rows:64px 1fr;height:100vh}
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
  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1360, height: 900 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.__ontoReady === true, { timeout: 15000 });
    await page.waitForFunction(() => !!document.querySelector('#h-explorer [data-act="new-from-template"]'), { timeout: 15000 }).catch(() => {});
    A('ready', true, '四区设计台就绪');

    const hasBtn = await page.evaluate(() => !!document.querySelector('#h-explorer [data-act="new-from-template"]'));
    A('tpl-btn', hasBtn, 'explorer 新建栏出「+ 从模板」按钮');

    // 打开模板对话框
    await page.click('#h-explorer [data-act="new-from-template"]');
    await page.waitForTimeout(500);
    const dlg = await page.evaluate(() => {
      const ov = document.querySelector('.o-dlg-overlay'); if (!ov) return null;
      return { hasSel: !!ov.querySelector('[data-k="tpl"]'), hasName: !!ov.querySelector('[data-k="apiName"]'), opts: [...ov.querySelectorAll('[data-k="tpl"] option')].map(o => o.textContent) };
    });
    A('dialog', dlg && dlg.hasSel && dlg.hasName, '弹出模板对话框（模板下拉 + apiName）', JSON.stringify(dlg && dlg.opts));
    A('has-consol', dlg && dlg.opts.some(o => /关账/.test(o)), '模板列表含「期末关账联动」');

    // 选 consolClose（index 0）+ 填 apiName + 使用模板
    await page.selectOption('.o-dlg-overlay [data-k="tpl"]', '0');
    await page.fill('.o-dlg-overlay [data-k="apiName"]', 'uiClose');
    await page.evaluate(() => { const b = [...document.querySelectorAll('.o-dlg-overlay button')].find(x => x.textContent.trim() === '使用模板'); if (b) b.click(); });
    await page.waitForTimeout(1500);

    // 编辑器应打开 uiClose，呈现两条副作用富块
    await page.waitForFunction(() => document.querySelectorAll('#h-property .o-fxflow').length >= 2, { timeout: 8000 }).catch(() => {});
    const blocks = await page.evaluate(() => document.querySelectorAll('#h-property .o-fxflow').length);
    A('editor-2blocks', blocks === 2, '编辑器自动呈现两条副作用富块', `flows=${blocks}`);
    const flowVal = await page.evaluate(() => (document.querySelector('#h-property [data-sef="flowDefKey"]') || {}).value || '');
    A('editor-flow', flowVal === 'consol_close', '起流程块 flowDefKey=consol_close', `val=${flowVal}`);
    const repVal = await page.evaluate(() => (document.querySelector('#h-property [data-sef="reportCode"]') || {}).value || '');
    A('editor-report', repVal === 'STAT_01_D', '生成报表块 reportCode=STAT_01_D', `val=${repVal}`);

    // API 校验：uiClose 落库两副作用
    const def = await api('/api/onto/v1/action-types/uiClose', 'GET');
    const kinds = ((def.data && def.data.sideEffects) || []).map(s => s.kind);
    A('api', kinds.includes('startBusinessProcess') && kinds.includes('computeReport'), 'API：uiClose 含 startBusinessProcess + computeReport', JSON.stringify(kinds));

    await page.screenshot({ path: path.resolve(__dirname, 'shots', 'onto_action_template.png') }).catch(() => {});
    console.log(`\n从模板新建动作 CDP：${_pass}/${_total} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); server.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
})();
