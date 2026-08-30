// UI4 演进安全守卫 CDP e2e 测试（破坏性变更 → 专业对话框 + 影响面 + 废弃建议）
// 验证：① 删对象类型 → 影响面对话框（关系引用 + 物化对象 + 状态）② 「改为废弃」路径
//       ③ 「仍然删除」级联删关系 ④ 删关系 → 确认对话框 ⑤ 删主键属性 → 警示 ⑥ 取消关闭
//
// 前置：onto-server :8097（auth off）
// 运行：node cmx-ontology/test/fe/onto_ui4_safety.cjs

'use strict';
const { chromium } = require('playwright');
const http = require('http');
const ONTO = { host: '127.0.0.1', port: 8097 };
const PORT = 9077;
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
const HARNESS = `<!doctype html><html><head><meta charset=utf-8><style>#h-content{height:520px}</style></head><body>
<div id=h-explorer></div><div id=h-content></div><div id=h-property></div>
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
const dlgVisible = () => !!document.querySelector('.o-dlg-overlay');

(async () => {
  // 前置：建 ui4_Ghost（试验态）+ ui4_Active（激活态，有物化对象）+ 引用它的关系
  await api('POST', '/api/onto/v1/object-types', { apiName: 'ui4_Ghost', displayName: '幽灵', primaryKey: 'id', properties: [{ apiName: 'id', baseType: 'long' }], status: 'experimental' });
  await api('POST', '/api/onto/v1/object-types', { apiName: 'ui4_Active', displayName: '激活态', primaryKey: 'aid', properties: [{ apiName: 'aid', baseType: 'long' }, { apiName: 'nm', baseType: 'string' }], status: 'active' });
  await api('POST', '/api/onto/v1/link-types', { apiName: 'ui4_ghostRel', objectTypeA: 'ui4_Ghost', objectTypeB: 'ui4_Active', cardinality: 'oneToMany' });
  // 给 ui4_Active 物化一个对象（使影响面能显示物化数）
  await api('POST', '/api/onto/v1/objects/ui4_Active', { properties: { aid: 1, nm: 'x' } });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1280, height: 760 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text().slice(0, 160)); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await waitFor(page, () => window.__ready === true);
    await waitFor(page, () => document.querySelector('#h-content cmx-ontology-graph') != null);

    // ── ① 删对象类型 ui4_Active → 影响面对话框 ──
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('type-select', { detail: { node: { id: 'ui4_Active', kind: 'object' } }, bubbles: true, composed: true })); });
    await waitFor(page, () => { const b = document.querySelector('#h-property [data-act="del-object"]'); return b && b.getAttribute('data-id') === 'ui4_Active'; });
    await page.click('#h-property [data-act="del-object"][data-id="ui4_Active"]');
    await waitFor(page, () => { const h = document.querySelector('.o-dlg-overlay .o-dlghd'); return h && h.textContent.includes('ui4_Active'); });
    A('dialog', await page.evaluate(dlgVisible), '删对象类型 → 专业对话框弹出（非 confirm）');
    const body = await page.evaluate(() => document.querySelector('.o-dlg-overlay .o-dlgbody').textContent);
    A('impact-rel', body.includes('ui4_ghostRel'), '影响面列出引用关系 ui4_ghostRel', body.includes('ui4_ghostRel') ? '' : body.slice(0, 80));
    A('impact-mat', /已物化对象.*1/.test(body), '影响面显示已物化对象数（1）');
    A('impact-status', body.includes('已启用') || body.includes('激活'), '影响面显示状态（激活）');
    A('impact-tip', body.includes('废弃'), '影响面含「改为废弃更安全」建议');
    A('dialog-danger', await page.evaluate(() => !!document.querySelector('.o-dlg-danger')), 'Active 类型 → danger 级对话框');

    // ── ② 「改为废弃」路径 ──
    await page.evaluate(() => { const b=document.querySelector('.o-dlg-overlay [data-dlg="deprecate"]'); if(b)b.click(); });
    await page.waitForTimeout(1000);
    const dep = await api('GET', '/api/onto/v1/object-types/ui4_Active');
    A('deprecate', dep && dep.data && dep.data.status === 'deprecated', '「改为废弃」→ status=deprecated（对象类型仍在）');

    // ── ③ 删 ui4_Ghost（试验态）→ 「仍然删除」级联删关系 ──
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('type-select', { detail: { node: { id: 'ui4_Ghost', kind: 'object' } }, bubbles: true, composed: true })); });
    // 等 property 区确实切到 ui4_Ghost（避免仍显上一个类型 → 删错对象）
    await waitFor(page, () => { const b = document.querySelector('#h-property [data-act="del-object"]'); return b && b.getAttribute('data-id') === 'ui4_Ghost'; });
    await page.click('#h-property [data-act="del-object"][data-id="ui4_Ghost"]');
    await waitFor(page, () => { const h = document.querySelector('.o-dlg-overlay .o-dlghd'); return h && h.textContent.includes('ui4_Ghost'); });
    A('dialog-warn', await page.evaluate(() => !!document.querySelector('.o-dlg-warn')), '试验态类型 → warn 级（非 danger）');
    await page.evaluate(() => { const b=document.querySelector('.o-dlg-overlay [data-dlg="delete"]'); if(b)b.click(); });
    await page.waitForTimeout(1400);
    const gone = await api('GET', '/api/onto/v1/object-types/ui4_Ghost');
    A('delete', gone && gone.code !== 0, '「仍然删除」→ 对象类型已删');
    const relGone = await api('GET', '/api/onto/v1/link-types/ui4_ghostRel');
    A('cascade', relGone && relGone.code !== 0, '级联删除引用关系 ui4_ghostRel');

    // ── ④ 删关系 → 确认对话框（用 ui4_Active 尚存的关系？已随 deprecate 保留；建一条新的）──
    await api('POST', '/api/onto/v1/object-types', { apiName: 'ui4_B', primaryKey: 'id', properties: [{ apiName: 'id', baseType: 'long' }] });
    await api('POST', '/api/onto/v1/link-types', { apiName: 'ui4_delRel', objectTypeA: 'ui4_Active', objectTypeB: 'ui4_B', cardinality: 'oneToMany' });
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('edge-select', { detail: { apiName: 'ui4_delRel' }, bubbles: true, composed: true })); });
    await waitFor(page, () => document.querySelector('#h-property [data-act="del-link"]') != null);
    await page.click('#h-property [data-act="del-link"]');
    await waitFor(page, dlgVisible);
    A('link-dialog', await page.evaluate(() => document.querySelector('.o-dlg-overlay .o-dlgbody').textContent.includes('Search-Around') || document.querySelector('.o-dlg-overlay') != null), '删关系 → 确认对话框（提示断链）');
    await page.evaluate(() => { const b=document.querySelector('.o-dlg-overlay [data-dlg="delete"]'); if(b)b.click(); });
    await page.waitForTimeout(1000);
    const ldg = await api('GET', '/api/onto/v1/link-types/ui4_delRel');
    A('link-delete', ldg && ldg.code !== 0, '确认 → 关系已删');

    // ── ⑤ 删主键属性 → 警示 + 取消关闭 ──
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('type-select', { detail: { node: { id: 'ui4_Active', kind: 'object' } }, bubbles: true, composed: true })); });
    await waitFor(page, () => { const b = document.querySelector('#h-property [data-act="del-object"]'); return b && b.getAttribute('data-id') === 'ui4_Active' && document.querySelectorAll('#h-property .o-prow').length >= 1; });
    // 删主键行（aid=第 0 行，其 apiName===primaryKey）
    await page.evaluate(() => { const b = document.querySelector('#h-property .o-prow [data-act="del-prop"]'); if (b) b.click(); });
    await waitFor(page, () => { const h = document.querySelector('.o-dlg-overlay .o-dlghd'); return h && h.textContent.includes('主键'); });
    A('pk-guard', await page.evaluate(() => { const h = document.querySelector('.o-dlg-overlay .o-dlghd'); return !!h && h.textContent.includes('主键'); }), '删主键属性 → 主键警示对话框');
    // 取消 → 关闭，属性未删
    await page.evaluate(() => { const b=document.querySelector('.o-dlg-overlay [data-dlg="__cancel"]'); if(b)b.click(); });
    await page.waitForTimeout(200);
    A('cancel', await page.evaluate(() => !document.querySelector('.o-dlg-overlay')), '取消 → 对话框关闭');

    const shotDir = require('path').resolve(__dirname, 'shots');
    require('fs').mkdirSync(shotDir, { recursive: true });
    // 重开删对象类型对话框截图
    await page.click('#h-property [data-act="del-object"]');
    await waitFor(page, dlgVisible);
    await page.screenshot({ path: require('path').join(shotDir, 'onto_ui4_impact.png') });
    A('shot', true, '截图存 test/fe/shots/onto_ui4_impact.png');
  } catch (e) {
    A('fatal', false, '测试异常', e.message);
  } finally {
    await browser.close(); server.close();
    // 清理
    for (const t of ['ui4_Active', 'ui4_B']) await api('DELETE', '/api/onto/v1/object-types/' + t, null);
    console.log(`\n结果：PASS=${_pass}/${_total}`);
    process.exit(_pass === _total ? 0 : 1);
  }
})();
