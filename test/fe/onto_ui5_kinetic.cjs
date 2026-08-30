// UI5 动能层（动作/函数编辑器）CDP e2e 测试
// 验证：① 选函数 → 可编辑 Inspector（runtime/kind/inputs/body）→ 改+存 round-trip
//       ② 选动作 → 可编辑 Inspector（参数/编辑规则/校验FEEL/副作用）→ 加校验+存 round-trip
//       ③ 新建函数/动作（explorer + 函数/+ 动作）④ 删除函数（专业对话框）
//
// 前置：onto-server :8097（auth off）
// 运行：node cmx-ontology/test/fe/onto_ui5_kinetic.cjs

'use strict';
const { chromium } = require('playwright');
const http = require('http');
const ONTO = { host: '127.0.0.1', port: 8097 };
const PORT = 9076;
let _pass = 0, _total = 0;
function A(id, ok, desc, detail) { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${desc}${detail ? '  :: ' + detail : ''}`); }
function api(method, path, body) {
  return new Promise((resolve, reject) => {
    const data = body ? JSON.stringify(body) : null;
    const req = http.request({ hostname: ONTO.host, port: ONTO.port, path, method, headers: { 'Content-Type': 'application/json', Accept: 'application/json', ...(data ? { 'Content-Length': Buffer.byteLength(data) } : {}) } }, (res) => { const ch = []; res.on('data', c => ch.push(c)); res.on('end', () => { try { resolve(JSON.parse(Buffer.concat(ch).toString())); } catch { resolve(null); } }); });
    req.on('error', reject); if (data) req.write(data); req.end();
  });
}
function startServer() {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = req.url.split('?')[0];
      if (url === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(HARNESS); return; }
      if (url.startsWith('/api/')) { const ch = []; req.on('data', c => ch.push(c)); req.on('end', () => { const b = ch.length ? Buffer.concat(ch) : null; const p = http.request({ hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}` } }, pr => { res.writeHead(pr.statusCode, pr.headers); pr.pipe(res); }); p.on('error', () => { res.writeHead(502); res.end(); }); if (b) p.write(b); p.end(); }); return; }
      res.statusCode = 404; res.end();
    });
    server.listen(PORT, () => resolve(server));
  });
}
const HARNESS = `<!doctype html><html><head><meta charset=utf-8><style>#h-explorer{width:280px;height:640px;overflow:auto}#h-property{width:360px;height:640px;overflow:auto}</style></head><body>
<div id=h-explorer></div><div id=h-content style=display:none></div><div id=h-property></div>
<script type=module>
const r=await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json());
const mod=await import(URL.createObjectURL(new Blob([r.data.source],{type:'text/javascript'})));
mod.configure({apiBase:''}); const d=mod.default; window.__mod=mod;
await d.views.explorer({host:document.getElementById('h-explorer')});
await d.views.content({host:document.getElementById('h-content')});
await d.views.property({host:document.getElementById('h-property')});
window.__ready=true;
</script></body></html>`;
async function waitFor(page, fn, t = 15000) { return page.waitForFunction(fn, { timeout: t }).catch(() => null); }

(async () => {
  // 前置：建 probeFn + probeAction
  await api('POST', '/api/onto/v1/functions', { apiName: 'probeFn', displayName: '探针函数', runtime: 'feel', kind: 'derivedProperty', inputs: [{ name: 'amount', type: 'decimal' }], output: { type: 'double' }, body: 'if amount > 1000 then 0.8 else 0.2' });
  await api('POST', '/api/onto/v1/action-types', { apiName: 'probeAction', displayName: '探针动作', parameters: [{ name: 'order', type: 'object', objectType: 'qa_Order' }], logic: [{ op: 'modifyObject', target: 'order' }], validations: [{ expression: 'amount > 0', message: '金额须正' }], sideEffects: [{ kind: 'startBusinessProcess', flowDefKey: 'reassign' }] });
  await api('DELETE', '/api/onto/v1/functions/ui5NewFn', null);
  await api('DELETE', '/api/onto/v1/action-types/ui5NewAct', null);

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 700, height: 760 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text().slice(0, 160)); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await waitFor(page, () => window.__ready === true);
    await waitFor(page, () => document.querySelector('#h-explorer [data-sel-id="probeFn"]') != null);

    // ── ① 函数 Inspector ──
    await page.click('#h-explorer [data-sel-id="probeFn"]');
    await waitFor(page, () => document.querySelector('#h-property [data-ff="runtime"]') != null);
    await page.waitForTimeout(400); // settle：等 selectElement 异步完成，避免编辑被后到的 refresh 覆盖
    const fnUi = await page.evaluate(() => {
      const q = (s) => document.querySelector('#h-property ' + s);
      return { runtime: q('[data-ff="runtime"]') ? q('[data-ff="runtime"]').value : '', kind: q('[data-ff="kind"]') ? q('[data-ff="kind"]').value : '', body: q('[data-ff="body"]') ? q('[data-ff="body"]').value : '', inputs: document.querySelectorAll('#h-property .o-fnrow [data-inf="name"]').length };
    });
    A('fn-inspector', !!fnUi.runtime && !!fnUi.kind, '函数 Inspector 出 runtime/kind 下拉');
    A('fn-body', fnUi.body.includes('amount'), '函数体载入 FEEL 表达式', fnUi.body.slice(0, 40));
    A('fn-inputs', fnUi.inputs === 1, '输入参数行载入（amount）', `inputs=${fnUi.inputs}`);

    // 改 runtime=rhai + body + 加一个输入 + 保存
    await page.evaluate(() => { document.querySelector('#h-property [data-ff="runtime"]').value = 'rhai'; document.querySelector('#h-property [data-ff="runtime"]').dispatchEvent(new Event('change', { bubbles: true })); });
    await page.waitForTimeout(150);
    await page.evaluate(() => { const b = document.querySelector('#h-property [data-ff="body"]'); b.value = 'let r = amount * 0.001; r'; b.dispatchEvent(new Event('input', { bubbles: true })); });
    await page.click('#h-property [data-act="fn-add-input"]');
    await page.waitForTimeout(150);
    await page.click('#h-property [data-act="save-function"]');
    let fnBack = null;
    for (let i = 0; i < 10; i++) { await page.waitForTimeout(300); fnBack = await api('GET', '/api/onto/v1/functions/probeFn'); if (fnBack && fnBack.data && fnBack.data.runtime === 'rhai') break; }
    A('fn-save', fnBack && fnBack.data && fnBack.data.runtime === 'rhai' && fnBack.data.inputs.length === 2, '函数保存 round-trip（runtime=rhai，2 输入）', `rt=${fnBack && fnBack.data ? fnBack.data.runtime : '?'} in=${fnBack && fnBack.data ? fnBack.data.inputs.length : '?'}`);

    // ── ② 动作 Inspector ──
    await page.click('#h-explorer [data-sel-id="probeAction"]');
    // 等动作 Inspector 完全载入（四段就位、种子校验 1 条），避免上一步 selectElement 异步settling 打断
    await waitFor(page, () => document.querySelectorAll('#h-property [data-vaf="expression"]').length === 1 && document.querySelector('#h-property [data-act="save-action"]') != null);
    const acUi = await page.evaluate(() => ({
      params: document.querySelectorAll('#h-property [data-paf="name"]').length,
      logic: document.querySelectorAll('#h-property [data-lof="op"]').length,
      vals: document.querySelectorAll('#h-property [data-vaf="expression"]').length,
      fx: document.querySelectorAll('#h-property [data-sef="kind"]').length,
    }));
    A('ac-inspector', acUi.params === 1 && acUi.logic === 1 && acUi.vals === 1 && acUi.fx === 1, '动作 Inspector 出参数/规则/校验/副作用四段', JSON.stringify(acUi));
    // 加一条校验 FEEL + 保存
    await page.click('#h-property [data-act="ac-add-val"]');
    await page.waitForTimeout(150);
    await page.evaluate(() => { const exs = document.querySelectorAll('#h-property [data-vaf="expression"]'); const last = exs[exs.length - 1]; last.value = 'region != null'; last.dispatchEvent(new Event('input', { bubbles: true })); });
    await page.click('#h-property [data-act="save-action"]');
    let acBack = null;
    for (let i = 0; i < 10; i++) { await page.waitForTimeout(300); acBack = await api('GET', '/api/onto/v1/action-types/probeAction'); if (acBack && acBack.data && acBack.data.validations.length === 2) break; }
    A('ac-save', acBack && acBack.data && acBack.data.validations.length === 2, '动作保存 round-trip（2 校验）', `vals=${acBack && acBack.data ? acBack.data.validations.length : '?'}`);
    A('ac-feel', acBack && acBack.data && acBack.data.validations.some(v => v.expression === 'region != null'), '新校验 FEEL 表达式落库');

    // ── ③ 新建函数（explorer + 函数；stub prompt）──
    await page.evaluate(() => { window.prompt = () => 'ui5NewFn'; });
    await page.click('#h-explorer [data-act="new-function"]');
    // 等新建 settle（reselect ui5NewFn → 函数 Inspector data-id=ui5NewFn）
    await waitFor(page, () => { const b = document.querySelector('#h-property [data-act="del-function"]'); return b && b.getAttribute('data-id') === 'ui5NewFn'; });
    const nf = await api('GET', '/api/onto/v1/functions/ui5NewFn');
    A('new-fn', nf && nf.data && nf.data.apiName === 'ui5NewFn', '新建函数落库（explorer + 函数）');

    // ── ④ 删除函数（专业对话框）──
    await page.click('#h-explorer [data-sel-id="probeFn"]');
    await waitFor(page, () => { const b = document.querySelector('#h-property [data-act="del-function"]'); return b && b.getAttribute('data-id') === 'probeFn'; });
    await page.click('#h-property [data-act="del-function"][data-id="probeFn"]');
    await waitFor(page, () => document.querySelector('.o-dlg-overlay') != null);
    A('del-dialog', await page.evaluate(() => !!document.querySelector('.o-dlg-overlay')), '删函数 → 专业对话框');
    await page.evaluate(() => { const b = document.querySelector('.o-dlg-overlay [data-dlg="delete"]'); if (b) b.click(); });
    // 轮询等待后端删除完成（doDelFunction 删后还有 loadAll+refreshAll ~900ms）
    let df = null;
    for (let i = 0; i < 8; i++) { await page.waitForTimeout(300); df = await api('GET', '/api/onto/v1/functions/probeFn'); if (df && df.code !== 0) break; }
    A('del-fn', df && df.code !== 0, '确认 → 函数已删');

    const shotDir = require('path').resolve(__dirname, 'shots');
    require('fs').mkdirSync(shotDir, { recursive: true });
    await page.click('#h-explorer [data-sel-id="probeAction"]');
    await waitFor(page, () => document.querySelector('#h-property [data-act="save-action"]') != null);
    await page.screenshot({ path: require('path').join(shotDir, 'onto_ui5_action.png') });
    A('shot', true, '截图存 test/fe/shots/onto_ui5_action.png');
  } catch (e) {
    A('fatal', false, '测试异常', e.message);
  } finally {
    await browser.close(); server.close();
    for (const f of ['probeFn', 'ui5NewFn']) await api('DELETE', '/api/onto/v1/functions/' + f, null);
    console.log(`\n结果：PASS=${_pass}/${_total}`);
    process.exit(_pass === _total ? 0 : 1);
  }
})();
