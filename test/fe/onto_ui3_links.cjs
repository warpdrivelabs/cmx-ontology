// UI3 关系直接操作 CDP e2e 测试（画布内联速建气泡 + 可编辑关系 Inspector + 自关联）
// 验证：① 拉线 link-add → 内联气泡出现（非 prompt）② 气泡字段（apiName/基数/角色）
//       ③ 创建关系 → 后端落库 + 画布出边 ④ 选中关系 → 可编辑 Inspector（改基数/角色 → 保存）
//       ⑤ 自关联（source===target）气泡标记层级 ⑥ 取消气泡
//
// 前置：onto-server :8097（auth off）；库中有 qa_Customer + qa_Order
// 运行：node cmx-ontology/test/fe/onto_ui3_links.cjs

'use strict';
const { chromium } = require('playwright');
const http = require('http');
const ONTO = { host: '127.0.0.1', port: 8097 };
const PORT = 9079;
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
const HARNESS = `<!doctype html><html><head><meta charset=utf-8><style>#h-content{height:640px}</style></head><body>
<div id=h-explorer></div><div id=h-content></div><div id=h-property></div>
<script type=module>
const r=await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json());
const src=r.data?r.data.source:r.source;
const mod=await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'})));
mod.configure({apiBase:''}); const d=mod.default; window.__mod=mod;
await d.views.explorer({host:document.getElementById('h-explorer')});
await d.views.content({host:document.getElementById('h-content')});
await d.views.property({host:document.getElementById('h-property')});
window.__ready=true;
</script></body></html>`;
async function waitFor(page, fn, t = 15000) { return page.waitForFunction(fn, { timeout: t }).catch(() => null); }

(async () => {
  // 前置：确保对象类型 + 清掉可能存在的测试关系
  await api('POST', '/api/onto/v1/object-types', { apiName: 'qa_Customer', displayName: '客户', primaryKey: 'id', properties: [{ apiName: 'id', baseType: 'long' }], status: 'active' });
  await api('POST', '/api/onto/v1/object-types', { apiName: 'qa_Order', displayName: '订单', primaryKey: 'oid', properties: [{ apiName: 'oid', baseType: 'long' }] });
  await api('DELETE', '/api/onto/v1/link-types/ui3_rel', null);

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1280, height: 820 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text().slice(0, 160)); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await waitFor(page, () => window.__ready === true);
    await waitFor(page, () => document.querySelector('#h-content cmx-ontology-graph') != null);
    await waitFor(page, () => { const el = document.querySelector('#h-content cmx-ontology-graph'); return el && el.shadowRoot && el.shadowRoot.querySelectorAll('.og-object').length >= 2; });

    // ① 模拟拉线：直接派发组件的 link-add（等价于拖 port 落到目标节点）
    await page.evaluate(() => {
      const el = document.querySelector('#h-content cmx-ontology-graph');
      el.dispatchEvent(new CustomEvent('link-add', { detail: { source: 'qa_Customer', target: 'qa_Order' }, bubbles: true, composed: true }));
    });
    await waitFor(page, () => document.querySelector('#h-content [data-role="link-bubble"]') != null);
    const bubbleShown = await page.evaluate(() => !!document.querySelector('#h-content [data-role="link-bubble"]'));
    A('bubble', bubbleShown, '拉线 → 内联速建气泡出现（非 prompt）');

    // ② 气泡字段齐 + apiName 自动建议
    const fields = await page.evaluate(() => {
      const b = document.querySelector('#h-content [data-role="link-bubble"]');
      return { api: !!b.querySelector('[data-lf="apiName"]'), card: !!b.querySelector('[data-lf="cardinality"]'), roleA: !!b.querySelector('[data-lf="roleA"]'), suggest: b.querySelector('[data-lf="apiName"]').value };
    });
    A('bubble-fields', fields.api && fields.card && fields.roleA, '气泡含 apiName/基数/角色字段');
    A('bubble-suggest', fields.suggest && fields.suggest.startsWith('qa') && fields.suggest.includes('Has'), 'apiName 自动建议（Has 拼接）', `suggest=${fields.suggest}`);

    // ③ 填 apiName=ui3_rel、基数=manyToMany → 创建关系
    await page.evaluate(() => {
      const b = document.querySelector('#h-content [data-role="link-bubble"]');
      b.querySelector('[data-lf="apiName"]').value = 'ui3_rel';
      b.querySelector('[data-lf="displayName"]').value = 'UI3关系';
      b.querySelector('[data-lf="cardinality"]').value = 'manyToMany';
      b.querySelector('[data-lf="roleA"]').value = 'places';
    });
    await page.click('#h-content [data-act="confirm-link"]');
    await page.waitForTimeout(1200);
    const created = await api('GET', '/api/onto/v1/link-types/ui3_rel');
    A('create', created && created.data && created.data.cardinality === 'manyToMany', '创建关系落库（基数 N:M）', `card=${created && created.data ? created.data.cardinality : '?'}`);
    A('create-role', created && created.data && created.data.roleA === 'places', '角色 roleA=places 落库');
    // 气泡关闭（等 loadAll+refreshAll 链完成，最长 ~1.5s）
    await waitFor(page, () => !document.querySelector('#h-content [data-role="link-bubble"]'), 4000);
    const bubbleGone = await page.evaluate(() => !document.querySelector('#h-content [data-role="link-bubble"]'));
    A('bubble-close', bubbleGone, '创建后气泡关闭');

    // ④ 选中关系 → 可编辑 Inspector；改基数为 oneToMany + 保存
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('edge-select', { detail: { apiName: 'ui3_rel' }, bubbles: true, composed: true })); });
    await waitFor(page, () => document.querySelector('#h-property [data-lf="cardinality"]') != null);
    const editable = await page.evaluate(() => !!document.querySelector('#h-property [data-lf="cardinality"]') && !!document.querySelector('#h-property [data-act="save-link"]'));
    A('link-inspector', editable, '关系 Inspector 可编辑（基数下拉 + 保存按钮）');
    await page.evaluate(() => { document.querySelector('#h-property [data-lf="cardinality"]').value = 'oneToMany'; document.querySelector('#h-property [data-lf="roleB"]').value = 'placedBy'; });
    await page.click('#h-property [data-act="save-link"]');
    await page.waitForTimeout(800);
    const edited = await api('GET', '/api/onto/v1/link-types/ui3_rel');
    A('link-edit', edited && edited.data && edited.data.cardinality === 'oneToMany' && edited.data.roleB === 'placedBy', '关系 Inspector 保存改动（基数→1:N，roleB=placedBy）', `card=${edited && edited.data ? edited.data.cardinality : '?'}`);

    // ⑤ 自关联：link-add source===target → 气泡标记层级
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('link-add', { detail: { source: 'qa_Customer', target: 'qa_Customer' }, bubbles: true, composed: true })); });
    await waitFor(page, () => document.querySelector('#h-content [data-role="link-bubble"]') != null);
    const selfMark = await page.evaluate(() => { const b = document.querySelector('#h-content [data-role="link-bubble"]'); return b.textContent.includes('自关联'); });
    A('self-rel', selfMark, '自关联气泡标记「层级」');

    // ⑥ 取消气泡
    await page.click('#h-content [data-act="cancel-link"]');
    await page.waitForTimeout(200);
    const canceled = await page.evaluate(() => !document.querySelector('#h-content [data-role="link-bubble"]'));
    A('cancel', canceled, '取消 → 气泡关闭');

    const shotDir = require('path').resolve(__dirname, 'shots');
    require('fs').mkdirSync(shotDir, { recursive: true });
    // 再开一次气泡截图
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); el.dispatchEvent(new CustomEvent('link-add', { detail: { source: 'qa_Customer', target: 'qa_Order' }, bubbles: true, composed: true })); });
    await page.waitForTimeout(300);
    await page.screenshot({ path: require('path').join(shotDir, 'onto_ui3_bubble.png') });
    A('shot', true, '截图存 test/fe/shots/onto_ui3_bubble.png');
  } catch (e) {
    A('fatal', false, '测试异常', e.message);
  } finally {
    await browser.close(); server.close();
    await api('DELETE', '/api/onto/v1/link-types/ui3_rel', null);
    console.log(`\n结果：PASS=${_pass}/${_total}`);
    process.exit(_pass === _total ? 0 : 1);
  }
})();
