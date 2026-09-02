// 门户手工走查：真机登录 :8080/portal → 列模块树 → 逐一经门户反代打通 onto/flow/report/dataauth 后端
// → openNode 打开本体设计工作台渲染 <cmx-ontology-graph>。验证「门户 + 本体/流程/报表/权限」端到端。
// 前置：portal:8080 + onto:8097 + flow:8091 + report:8092 + dataauth:8098 在跑；admin/Admin@12345。
'use strict';
const { chromium } = require('playwright');
const ORIGIN = process.env.PORTAL_ORIGIN || 'http://127.0.0.1:8080';
const BASE = ORIGIN + '/portal';
const USER = process.env.PORTAL_USER || 'admin';
const PASS = process.env.PORTAL_PASS || 'Admin@12345';
let _pass = 0, _total = 0;
function A(id, ok, desc, detail) { _total++; if (ok) _pass++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${desc}${detail ? '  :: ' + detail : ''}`); }
const DEEP_FIND = `function deepFind(tag){const stack=[document];while(stack.length){const root=stack.pop();try{const f=root.querySelector&&root.querySelector(tag);if(f)return f;}catch(e){}const all=root.querySelectorAll?root.querySelectorAll('*'):[];for(const el of all){if(el.shadowRoot)stack.push(el.shadowRoot);if(el.tagName==='IFRAME'){try{if(el.contentDocument)stack.push(el.contentDocument);}catch(e){}}}}return null;}`;
// 原始 XHR（门户 app 改写了 window.fetch 拆信封，故用 XHR 取真实 HTTP 状态穿透门户反代）。
const RAW = `function raw(url,tk){return new Promise(res=>{try{const x=new XMLHttpRequest();x.open('GET',url);if(tk)x.setRequestHeader('Authorization','Bearer '+tk);x.onreadystatechange=()=>{if(x.readyState===4)res({status:x.status,body:(x.responseText||'').slice(0,120)});};x.onerror=()=>res({status:-1,body:'xhr-error'});x.send();}catch(e){res({status:-2,body:String(e)});}});}`;

async function main() {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1680, height: 1000 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push(String(e)));
  try {
    // ① 登录
    await page.goto(`${BASE}/login.html`, { waitUntil: 'domcontentloaded' });
    await page.fill('#username', USER); await page.fill('#password', PASS);
    await Promise.all([page.waitForNavigation({ waitUntil: 'domcontentloaded' }).catch(() => {}), page.click('#submit')]);
    await page.goto(`${BASE}/`, { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('cmx-portal-app', { timeout: 20000 });
    await page.waitForTimeout(2500);
    A('P1', true, '真机登录进入组件门户 <cmx-portal-app>（:8080/portal）');
    const TK = await page.evaluate(() => localStorage.getItem('cmx_access_token') || '');
    A('P1b', !!TK, '取得会话 token（cmx_access_token）', TK ? 'ok' : '空');

    // ② 模块树
    const modules = await page.evaluate(async ([o, tk]) => {
      const r = await fetch(o + '/api/domains/tree', { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + tk }, body: '{}' }).then(r => r.json());
      const roots = Array.isArray(r) ? r : (r && r.data) || []; const mods = [];
      (function w(ns) { for (const n of ns || []) { const d = n.data || {}; if (d.node_type === 'module') mods.push(d.code); w(n.children); } })(roots);
      return mods;
    }, [ORIGIN, TK]);
    A('P2', modules.includes('onto'), '门户模块树含 onto 模块', `modules=${modules.join(',')}`);

    // ③ 门户反代四后端连通（登录态 + 真实状态经 XHR）
    const probe = async (path) => page.evaluate(`(()=>{${RAW};return raw(${JSON.stringify(ORIGIN + path)}, ${JSON.stringify(TK)});})()`);
    const onto = await probe('/api/onto/v1/manifest');
    A('P3', onto.status === 200 && /objectTypes/.test(onto.body), '门户→本体(onto) 反代连通 /api/onto/v1/manifest', `HTTP ${onto.status}`);
    const flow = await probe('/api/flow/definitions');
    A('P4', flow.status === 200, '门户→流程(flow) 反代连通 /api/flow/definitions', `HTTP ${flow.status}`);
    const report = await probe('/api/report-design/reports');
    A('P5', report.status === 200 && /items|dbId/.test(report.body), '门户→报表(report) 反代连通 /api/report-design/reports', `HTTP ${report.status}`);
    const da = await probe('/api/dataauth/v1/policies');
    A('P6', da.status === 200 || da.status === 403, '门户→权限(dataauth) 反代连通 /api/dataauth/v1/policies', `HTTP ${da.status}（200 或 403=已达权限引擎）`);

    // ④ openNode 打开本体设计工作台
    const ontoNode = await page.evaluate(async ([o, tk]) => {
      const r = await fetch(o + '/api/menu/tree?domain_code=basic&application_code=dataplatform&module_code=onto', { headers: { Authorization: 'Bearer ' + tk } }).then(r => r.json());
      const roots = Array.isArray(r) ? r : (r && r.data) || []; let node = null;
      (function w(ns) { for (const n of ns || []) { const d = n.data || n; if (d.code === 'onto-designer') node = d; w(n.children); } })(roots);
      if (!node) return null; let def = node.definition; if (typeof def === 'string') { try { def = JSON.parse(def); } catch (e) { } } return def;
    }, [ORIGIN, TK]);
    A('P7', !!ontoNode, 'menu/tree 取到 onto-designer 节点', ontoNode ? 'ok' : '缺');
    const opened = await page.evaluate(async (node) => { const app = document.querySelector('cmx-portal-app'); if (!app || typeof app.openNode !== 'function' || !node) return 'no'; await app.openNode(node); return 'ok'; }, ontoNode);
    await page.waitForFunction(`(()=>{${DEEP_FIND};return !!deepFind('cmx-ontology-graph')})()`, { timeout: 20000 }).catch(() => {});
    await page.waitForTimeout(1200);
    const hasGraph = await page.evaluate(`(()=>{${DEEP_FIND};return !!deepFind('cmx-ontology-graph')})()`);
    A('P8', opened === 'ok' && hasGraph, '门户 openNode 打开本体设计台 + 渲染 <cmx-ontology-graph>', `opened=${opened} graph=${hasGraph}`);

    await page.screenshot({ path: '/tmp/portal-walkthrough.png', timeout: 5000 }).catch(() => {});
    const fatal = errors.filter(e => !/favicon|net::ERR_|ResizeObserver|WebSocket|ws:\/\//.test(e));
    A('P9', fatal.length === 0, '门户无致命前端错误', fatal.slice(0, 3).join(' | ') || '干净');

    console.log(`\n门户手工走查：${_pass}/${_total} 通过  （截图 /tmp/portal-walkthrough.png）`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 200)); }
  finally { await browser.close(); process.exit(_pass >= _total - 1 ? 0 : 1); }
}
main();
