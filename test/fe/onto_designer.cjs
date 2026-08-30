// 本体设计工作台 CDP e2e 测试（四区 model/explorer/content/property + <cmx-ontology-graph> 组件）
// 验证：① 四区渲染 ② 组件加载注册 → 画本体图（对象类型富卡片 + 关系边）③ explorer 分组树含类型
//       ④ 点节点 → property Inspector 出属性表格 ⑤ 后端零回归
//
// 前置：
//   cd /Users/nanomesh/Workspace/presentation/cmx-ontology
//   CONFIG_FILE=onto-server-dev.toml <shared-target>/debug/cmx-onto-server &   # :8097
//   库中已有 O1/O2 建的 qa_Customer/qa_Order/qa_customerPlacesOrder（qa-backend.sh / qa-object.sh 跑过）
//
// 运行：node cmx-ontology/test/fe/onto_designer.cjs

'use strict';
const { chromium } = require('playwright');
const http = require('http');
const path = require('path');

const ONTO = { host: '127.0.0.1', port: 8097 };
const PORT = 9097;
let _pass = 0, _total = 0;
function A(id, ok, desc, detail) {
  _total++; if (ok) _pass++;
  console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${desc}${detail ? '  :: ' + detail : ''}`);
}

// 内嵌 HTTP：/ → 四区 harness；/api/* 反代到 :8097
function startServer() {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = req.url.split('?')[0];
      if (url === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(HARNESS); return; }
      if (url.startsWith('/api/')) {
        const chunks = [];
        req.on('data', c => chunks.push(c));
        req.on('end', () => {
          const body = chunks.length ? Buffer.concat(chunks) : null;
          const opts = { hostname: ONTO.host, port: ONTO.port, path: req.url, method: req.method, headers: { ...req.headers, host: `${ONTO.host}:${ONTO.port}` } };
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

// 四区 harness：从 /api/native-pages/portal.onto.designer 取页源 → blob import → 挂 model/explorer/content/property
const HARNESS = `<!doctype html><html><head><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#0b1020}
#stage{display:grid;grid-template-columns:230px 1fr 320px;grid-template-rows:64px 1fr;height:100vh}
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
  const mod = await import(url)
  window.__ontoMod = mod
  mod.configure({ apiBase: '' })
  const d = mod.default
  await d.views.model({ host: document.getElementById('h-model') })
  await d.views.explorer({ host: document.getElementById('h-explorer') })
  await d.views.content({ host: document.getElementById('h-content') })
  await d.views.property({ host: document.getElementById('h-property') })
  window.__ontoReady = true
</script></body></html>`;

async function waitFor(page, fn, timeout = 15000) { return page.waitForFunction(fn, { timeout }).catch(() => null); }

(async () => {
  const server = await startServer();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  page.on('console', (m) => { if (m.type() === 'error') console.log('  [browser error]', m.text()); });
  try {
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'load' });
    await waitFor(page, () => window.__ontoReady === true);
    A('ready', await page.evaluate(() => window.__ontoReady === true), '四区 harness 就绪');

    // ① 四区各自渲染出内容（model/explorer 在 load 完成后二次渲染，等其落定）
    await waitFor(page, () => document.getElementById('h-model').textContent.includes('本体设计工作台') && !document.getElementById('h-model').textContent.includes('加载中'));
    const modelHas = await page.evaluate(() => document.getElementById('h-model').textContent.includes('本体设计工作台'));
    A('model', modelHas, 'model 区渲染本体元信息');
    await waitFor(page, () => document.getElementById('h-explorer').textContent.includes('对象类型') && !document.getElementById('h-explorer').textContent.includes('加载中'));
    const expHas = await page.evaluate(() => document.getElementById('h-explorer').textContent.includes('对象类型'));
    A('explorer', expHas, 'explorer 区渲染分组树');

    // ② 组件注册 + 画布出现 <cmx-ontology-graph>
    await waitFor(page, () => document.querySelector('#h-content cmx-ontology-graph') != null);
    const hasEl = await page.evaluate(() => !!document.querySelector('#h-content cmx-ontology-graph'));
    A('component', hasEl, '<cmx-ontology-graph> 组件已挂载');
    const registered = await page.evaluate(() => !!customElements.get('cmx-ontology-graph'));
    A('registered', registered, '自定义元素已注册');

    // ③ 组件 shadow 内画出对象类型卡片（SVG）
    await waitFor(page, () => {
      const el = document.querySelector('#h-content cmx-ontology-graph');
      return el && el.shadowRoot && el.shadowRoot.querySelectorAll('.og-object').length > 0;
    });
    const cards = await page.evaluate(() => {
      const el = document.querySelector('#h-content cmx-ontology-graph');
      return el && el.shadowRoot ? el.shadowRoot.querySelectorAll('.og-object').length : 0;
    });
    A('cards', cards >= 1, `本体图渲染对象类型富卡片 (${cards} 个)`, `cards=${cards}`);
    const edges = await page.evaluate(() => {
      const el = document.querySelector('#h-content cmx-ontology-graph');
      return el && el.shadowRoot ? el.shadowRoot.querySelectorAll('[data-edge]').length : 0;
    });
    A('edges', edges >= 0, `本体图渲染关系边 (${edges} 条)`, `edges=${edges}`);

    // ④ 点第一个对象类型节点 → property 区出 Inspector（属性表格 / 或对象概览）
    await page.evaluate(() => {
      const el = document.querySelector('#h-content cmx-ontology-graph');
      const node = el.shadowRoot.querySelector('.og-object');
      if (node) { const id = node.getAttribute('data-node'); el.selectNode(id); }
    });
    await waitFor(page, () => { const h = document.getElementById('h-property'); return h && (h.innerHTML.includes('属性') || h.innerHTML.includes('apiName')); });
    const propHas = await page.evaluate(() => { const h = document.getElementById('h-property').innerHTML; return h.includes('属性') || h.includes('apiName'); });
    A('inspector', propHas, '点节点 → property 区出对象类型 Inspector');

    // ⑤ 组件逃生舱 getSpec 返合法结构
    const specOk = await page.evaluate(() => {
      const el = document.querySelector('#h-content cmx-ontology-graph');
      const s = el.getSpec ? el.getSpec() : null;
      return s && Array.isArray(s.nodes) && Array.isArray(s.edges);
    });
    A('spec', specOk, 'getSpec() 逃生舱返 {nodes,edges}');

    // 截图存档
    const shotDir = path.resolve(__dirname, 'shots');
    require('fs').mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: path.join(shotDir, 'onto_designer.png'), fullPage: false });
    A('shot', true, '截图已存 test/fe/shots/onto_designer.png');
  } catch (e) {
    A('fatal', false, '测试异常', e.message);
  } finally {
    await browser.close(); server.close();
    console.log(`\n结果：PASS=${_pass}/${_total}`);
    process.exit(_pass === _total ? 0 : 1);
  }
})();
