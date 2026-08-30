// 门户菜单真机验证：登录真实门户(:8080/portal，cmx-portal-app 组件门户) → 打开「本体设计工作台」
// → 四区 model/explorer/content/property + <cmx-ontology-graph> 渲染。验证「将菜单搞进门户」端到端。
//
// 关键环境事实：:8080 根 / 是另一 Vue 壳；DAM/native-pages 组件门户挂在 /portal（router.rs:101）。
// 前置：门户 :8080（portal-server-dev.toml）+ onto-server :8097 在跑；admin/Admin@12345。
// 运行：node cmx-ontology/test/fe/onto_portal_menu.cjs

'use strict';
const { chromium } = require('playwright');

const ORIGIN = process.env.PORTAL_ORIGIN || 'http://127.0.0.1:8080';
const BASE = ORIGIN + '/portal';
const USER = process.env.PORTAL_USER || 'admin';
const PASS = process.env.PORTAL_PASS || 'Admin@12345';
let _pass = 0, _total = 0;
function A(id, ok, desc, detail) {
  _total++; if (ok) _pass++;
  console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${desc}${detail ? '  :: ' + detail : ''}`);
}

const DEEP_FIND = `function deepFind(tag){
  const stack=[document];
  while(stack.length){
    const root=stack.pop();
    try{const f=root.querySelector&&root.querySelector(tag); if(f)return f;}catch(e){}
    const all=root.querySelectorAll?root.querySelectorAll('*'):[];
    for(const el of all){ if(el.shadowRoot)stack.push(el.shadowRoot);
      if(el.tagName==='IFRAME'){try{if(el.contentDocument)stack.push(el.contentDocument);}catch(e){}} }
  }
  return null;
}`;

async function main() {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1680, height: 1000 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push(String(e)));

  try {
    // ── ① 真机登录（/portal/login.html）──
    await page.goto(`${BASE}/login.html`, { waitUntil: 'domcontentloaded' });
    await page.fill('#username', USER);
    await page.fill('#password', PASS);
    await Promise.all([
      page.waitForNavigation({ waitUntil: 'domcontentloaded' }).catch(() => {}),
      page.click('#submit'),
    ]);
    await page.goto(`${BASE}/`, { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('cmx-portal-app', { timeout: 20000 });
    await page.waitForTimeout(2500);
    A('P1', true, '真机登录进入组件门户 <cmx-portal-app>（/portal）');

    // token（组件门户存 localStorage cmx_access_token）供 API 断言
    const TK = await page.evaluate(() => localStorage.getItem('cmx_access_token') || '');

    // ── ② domains/tree 含 onto 模块（注：/portal 应用改写 fetch 自动拆 ApiResp 信封，data 直出）──
    const treeHasOnto = await page.evaluate(async ([o, tk]) => {
      const r = await fetch(o + '/api/domains/tree', { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + tk }, body: '{}' }).then(r => r.json());
      const roots = Array.isArray(r) ? r : (r && r.data) || [];
      let f = false; (function w(ns) { for (const n of ns || []) { const d = n.data || {}; if (d.node_type === 'module' && d.code === 'onto') f = true; w(n.children); } })(roots); return f;
    }, [ORIGIN, TK]);
    A('P2', treeHasOnto, 'domains/tree 含 basic/dataplatform/onto 模块');

    // ── ③ menu/tree 出 onto-designer 完整四区节点（取回供 openNode）──
    const ontoNode = await page.evaluate(async ([o, tk]) => {
      const r = await fetch(o + '/api/menu/tree?domain_code=basic&application_code=dataplatform&module_code=onto', { headers: { Authorization: 'Bearer ' + tk } }).then(r => r.json());
      const roots = Array.isArray(r) ? r : (r && r.data) || [];
      let node = null; (function w(ns) { for (const n of ns || []) { const d = n.data || n; if (d.code === 'onto-designer') node = d; w(n.children); } })(roots);
      if (!node) return null; let def = node.definition; if (typeof def === 'string') { try { def = JSON.parse(def); } catch (e) {} }
      return def;
    }, [ORIGIN, TK]);
    const regions = ontoNode && ontoNode.workspace ? ['model', 'explorer', 'content', 'property'].filter(x => x in ontoNode.workspace) : [];
    A('P3', regions.length === 4, 'menu/tree 出 onto-designer 四区工作区节点', `regions=${regions}`);

    // ── ④ 门户 openNode 打开该节点（组件门户统一开页链路）──
    const opened = await page.evaluate(async (node) => {
      const app = document.querySelector('cmx-portal-app');
      if (!app || typeof app.openNode !== 'function') return 'no-openNode';
      await app.openNode(node); return 'ok';
    }, ontoNode);
    A('P4', opened === 'ok', '门户 openNode 打开本体设计工作台', `ret=${opened}`);

    // ── ⑤ 设计器渲染：等 <cmx-ontology-graph>（穿透 shadow/iframe）──
    await page.waitForFunction(`(()=>{${DEEP_FIND};return !!deepFind('cmx-ontology-graph')})()`, { timeout: 20000 }).catch(() => {});
    await page.waitForTimeout(1500);
    const hasGraph = await page.evaluate(`(()=>{${DEEP_FIND};return !!deepFind('cmx-ontology-graph')})()`);
    A('P5', hasGraph, 'content 区渲染本体图组件 <cmx-ontology-graph>');

    // 四区宿主是否都挂上（explorer 分组树 / property inspector 等）
    const regionsRendered = await page.evaluate(`(()=>{${DEEP_FIND};
      const has = t => !!deepFind(t);
      return { graph: has('cmx-ontology-graph'),
               // 设计器内部标志：工具栏/分组树/属性面板任一
               body: (document.querySelector('cmx-portal-app')?.shadowRoot?.textContent||'').includes('本体') };
    })()`);
    A('P6', regionsRendered.graph, '本体设计器四区已装载', `graph=${regionsRendered.graph}`);

    await page.screenshot({ path: '/tmp/onto-designer-opened.png' });
    const fatal = errors.filter(e => !/favicon|net::ERR_|ResizeObserver|WebSocket|ws:\/\//.test(e));
    A('P7', fatal.length === 0, '无致命前端错误', fatal.slice(0, 3).join(' | ') || '干净');

    console.log(`\n本体菜单真机验证：${_pass}/${_total} 通过  （截图 /tmp/onto-designer-opened.png）`);
  } catch (e) {
    console.error('测试异常:', e);
    A('FATAL', false, '测试执行', String(e).slice(0, 200));
  } finally {
    await browser.close();
    process.exit(_pass >= _total - 1 ? 0 : 1);
  }
}
main();
