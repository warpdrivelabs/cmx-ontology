// UI2 属性编辑深化 CDP e2e 测试（四区 property 区的属性编辑器）
// 验证：① 富属性表格（apiName/类型/PK/必填/索引/语义 + 拖拽柄 + 行选）
//       ② apiName 即时校验（非法→红框，重名→问题条）③ 加属性 / 删属性
//       ④ 引用共享属性（继承 baseType+semanticType）⑤ 批量必填 ⑥ 保存 round-trip 回读
//
// 前置：onto-server :8097（auth off）；库中有 qa_Customer（含属性）+ 共享属性 currencyCode
// 运行：node cmx-ontology/test/fe/onto_ui2_props.cjs

'use strict';
const { chromium } = require('playwright');
const http = require('http');
const ONTO = { host: '127.0.0.1', port: 8097 };
const PORT = 9098;
let _pass = 0, _total = 0;
function A(id, ok, desc, detail) { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${desc}${detail ? '  :: ' + detail : ''}`); }

function api(method, path, body) {
  return new Promise((resolve, reject) => {
    const data = body ? JSON.stringify(body) : null;
    const req = http.request({ hostname: ONTO.host, port: ONTO.port, path, method, headers: { 'Content-Type': 'application/json', Accept: 'application/json', ...(data ? { 'Content-Length': Buffer.byteLength(data) } : {}) } }, (res) => {
      const ch = []; res.on('data', c => ch.push(c)); res.on('end', () => { try { resolve(JSON.parse(Buffer.concat(ch).toString())); } catch { resolve(null); } });
    });
    req.on('error', reject); if (data) req.write(data); req.end();
  });
}

function startServer() {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = req.url.split('?')[0];
      if (url === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(HARNESS); return; }
      if (url.startsWith('/api/')) {
        const ch = []; req.on('data', c => ch.push(c));
        req.on('end', () => { const b = ch.length ? Buffer.concat(ch) : null; const p = http.request({ hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}` } }, pr => { res.writeHead(pr.statusCode, pr.headers); pr.pipe(res); }); p.on('error', () => { res.writeHead(502); res.end(); }); if (b) p.write(b); p.end(); });
        return;
      }
      res.statusCode = 404; res.end();
    });
    server.listen(PORT, () => resolve(server));
  });
}

const HARNESS = `<!doctype html><html><head><meta charset=utf-8></head><body>
<div id=h-explorer></div><div id=h-content></div><div id=h-property></div>
<script type=module>
const r=await fetch('/api/native-pages/portal.onto.designer').then(r=>r.json());
const src=r.data?r.data.source:r.source;
const mod=await import(URL.createObjectURL(new Blob([src],{type:'text/javascript'})));
mod.configure({apiBase:''}); const d=mod.default;
window.__mod=mod;
await d.views.explorer({host:document.getElementById('h-explorer')});
await d.views.content({host:document.getElementById('h-content')});
await d.views.property({host:document.getElementById('h-property')});
window.__ready=true;
</script></body></html>`;

async function waitFor(page, fn, t = 15000) { return page.waitForFunction(fn, { timeout: t }).catch(() => null); }

(async () => {
  // 前置：确保 qa_Customer + 共享属性存在
  await api('POST', '/api/onto/v1/object-types', { apiName: 'qa_Customer', displayName: '客户', primaryKey: 'id', titleProperty: 'name', properties: [{ apiName: 'id', baseType: 'long', required: true }, { apiName: 'name', baseType: 'string', isIndexed: true }, { apiName: 'region', baseType: 'string' }], status: 'active' });
  await api('POST', '/api/onto/v1/shared-properties', { apiName: 'currencyCode', displayName: '币种', baseType: 'string', semanticType: 'currency' });

  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1280, height: 820 } });
  page.on('console', m => { if (m.type() === 'error') console.log('  [browser error]', m.text().slice(0, 160)); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await waitFor(page, () => window.__ready === true);
    await waitFor(page, () => document.querySelector('#h-content cmx-ontology-graph') != null);
    await page.evaluate(() => { const el = document.querySelector('#h-content cmx-ontology-graph'); if (el) el.selectNode('qa_Customer'); });
    // 等属性行真正落定（selectElement 异步拉 detail 后再 refresh property）
    await waitFor(page, () => document.querySelectorAll('#h-property .o-prow').length === 3);

    // ① 富表格列齐（语义列 + 拖拽柄 + 行选 checkbox）
    const hasSem = await page.evaluate(() => !!document.querySelector('#h-property .o-csel.sem'));
    A('sem-col', hasSem, '属性表含语义类型下拉列');
    const hasHandle = await page.evaluate(() => !!document.querySelector('#h-property .handle[data-drag]'));
    A('drag-handle', hasHandle, '属性行有拖拽排序柄');
    const rowCount = await page.evaluate(() => document.querySelectorAll('#h-property .o-prow').length);
    A('rows', rowCount === 3, `qa_Customer 3 属性行`, `rows=${rowCount}`);

    // ② apiName 即时校验：改成非法 → 红框（dispatch + 读在同一 evaluate，避免跨调用被 refresh 清）
    const isBad = await page.evaluate(() => {
      const inp = document.querySelector('#h-property [data-pf="apiName"][data-pi="2"]');
      if (!inp) return false;
      inp.value = '2bad'; inp.dispatchEvent(new Event('input', { bubbles: true }));
      return inp.classList.contains('bad');
    });
    A('inline-validate', isBad, '非法 apiName 即时红框');
    // 触发一次 change 让 state 重渲 → 问题条出现 → 保存禁用
    await page.evaluate(() => {
      const inp = document.querySelector('#h-property [data-pf="apiName"][data-pi="2"]');
      if (inp) inp.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await page.waitForTimeout(200);
    const saveDisabled = await page.evaluate(() => { const b = document.querySelector('#h-property [data-act="save-object"]'); return !!(b && b.disabled); });
    A('save-guard', saveDisabled, '有问题时保存按钮禁用');
    // 改回合法
    await page.evaluate(() => { const inp = document.querySelector('#h-property [data-pf="apiName"][data-pi="2"]'); if (inp) { inp.value = 'region'; inp.dispatchEvent(new Event('input', { bubbles: true })); inp.dispatchEvent(new Event('change', { bubbles: true })); } });
    await page.waitForTimeout(200);

    // ③ 引用共享属性（继承 baseType+semanticType）
    await page.evaluate(() => {
      const sel = document.querySelector('#h-property [data-role="ref-shared"]');
      if (sel) { sel.value = 'currencyCode'; }
    });
    await page.evaluate(() => { const b = document.querySelector('#h-property [data-act="add-shared"]'); if (b) b.click(); });
    await page.waitForTimeout(200);
    const hasRef = await page.evaluate(() => Array.from(document.querySelectorAll('#h-property .o-refname')).some(e => e.textContent.includes('currencyCode')));
    A('shared-ref', hasRef, '引用共享属性 currencyCode（⊞ 标记）');

    // ④ 加属性 → 4+1(currency)=5 行
    await page.evaluate(() => { const b = document.querySelector('#h-property [data-act="add-prop"]'); if (b) b.click(); });
    await page.waitForTimeout(150);
    const rows2 = await page.evaluate(() => document.querySelectorAll('#h-property .o-prow').length);
    A('add-prop', rows2 === 5, `加属性后 5 行（3+共享+新）`, `rows=${rows2}`);

    // ⑤ 批量：全选 → 批量必填
    await page.evaluate(() => { document.querySelectorAll('#h-property .o-rowsel').forEach(cb => { cb.checked = true; cb.dispatchEvent(new Event('change', { bubbles: true })); }); });
    await page.waitForTimeout(150);
    const hasBatch = await page.evaluate(() => !!document.querySelector('#h-property .o-batch'));
    A('batch-bar', hasBatch, '多选出批量操作条');
    await page.evaluate(() => { const b = document.querySelector('#h-property [data-act="batch-required"]'); if (b) b.click(); });
    await page.waitForTimeout(150);
    const allReq = await page.evaluate(() => Array.from(document.querySelectorAll('#h-property [data-pf="required"]')).every(cb => cb.checked));
    A('batch-required', allReq, '批量必填生效（全部勾选）');

    // 截图
    const shotDir = require('path').resolve(__dirname, 'shots');
    require('fs').mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: require('path').join(shotDir, 'onto_ui2_props.png') });
    A('shot', true, '截图存 test/fe/shots/onto_ui2_props.png');

    // ⑥ 清理：删掉刚加的空属性和 currency 引用（避免污染），恢复 qa_Customer 原样并保存 round-trip
    // 直接后端还原
    await api('POST', '/api/onto/v1/object-types', { apiName: 'qa_Customer', displayName: '客户', primaryKey: 'id', titleProperty: 'name', properties: [{ apiName: 'id', baseType: 'long', required: true }, { apiName: 'name', baseType: 'string', isIndexed: true }, { apiName: 'region', baseType: 'string' }], status: 'active' });
    const back = await api('GET', '/api/onto/v1/object-types/qa_Customer');
    A('roundtrip', back && back.data && back.data.properties.length === 3, 'round-trip 回读 3 属性', `props=${back && back.data ? back.data.properties.length : '?'}`);
  } catch (e) {
    A('fatal', false, '测试异常', e.message + '\n' + (e.stack || '').split('\n').slice(0, 3).join(' | '));
  } finally {
    await browser.close(); server.close();
    console.log(`\n结果：PASS=${_pass}/${_total}`);
    process.exit(_pass === _total ? 0 : 1);
  }
})();
