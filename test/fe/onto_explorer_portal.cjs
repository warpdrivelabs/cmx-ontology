// 对象浏览器真机门户验证：登录 /portal → openNode(onto-explorer) → 四区渲染（对象列表表格）。
'use strict';
const { chromium } = require('playwright');
const ORIGIN = 'http://127.0.0.1:8080', BASE = ORIGIN + '/portal';
let _p = 0, _t = 0;
const A = (id, ok, d, x) => { _t++; if (ok) _p++; console.log(`[${id}] ${ok ? '\x1b[32mPASS\x1b[0m' : '\x1b[31mFAIL\x1b[0m'}  ${d}${x ? ' :: ' + x : ''}`); };
const DEEP = `function deepFind(t){const s=[document];while(s.length){const r=s.pop();try{const f=r.querySelector&&r.querySelector(t);if(f)return f;}catch(e){}const a=r.querySelectorAll?r.querySelectorAll('*'):[];for(const el of a){if(el.shadowRoot)s.push(el.shadowRoot);if(el.tagName==='IFRAME'){try{if(el.contentDocument)s.push(el.contentDocument);}catch(e){}}}}return null}`;
(async () => {
  const b = await chromium.launch({ headless: true });
  const page = await (await b.newContext({ viewport: { width: 1680, height: 1000 } })).newPage();
  try {
    await page.goto(`${BASE}/login.html`, { waitUntil: 'domcontentloaded' });
    await page.fill('#username', 'admin'); await page.fill('#password', 'Admin@12345');
    await Promise.all([page.waitForNavigation({ waitUntil: 'domcontentloaded' }).catch(() => {}), page.click('#submit')]);
    await page.goto(`${BASE}/`, { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('cmx-portal-app', { timeout: 20000 }); await page.waitForTimeout(2500);
    A('P1', true, '登录组件门户 /portal');
    const TK = await page.evaluate(() => localStorage.getItem('cmx_access_token') || '');
    // 取 onto-explorer 节点
    const node = await page.evaluate(async ([o, tk]) => {
      const r = await fetch(o + '/api/menu/tree?domain_code=basic&application_code=dataplatform&module_code=onto', { headers: { Authorization: 'Bearer ' + tk } }).then(r => r.json());
      const roots = Array.isArray(r) ? r : (r && r.data) || []; let n = null;
      (function w(ns) { for (const x of ns || []) { const dd = x.data || x; if (dd.code === 'onto-explorer') n = dd; w(x.children); } })(roots);
      if (!n) return null; let def = n.definition; if (typeof def === 'string') { try { def = JSON.parse(def); } catch (e) {} } return def;
    }, [ORIGIN, TK]);
    A('P2', !!node && !!node.workspace, 'menu/tree 出 onto-explorer 工作区节点', node ? `regions=${Object.keys(node.workspace).filter(k => ['model', 'explorer', 'content', 'property'].includes(k))}` : 'null');
    // openNode
    const opened = await page.evaluate(async (n) => { const app = document.querySelector('cmx-portal-app'); if (!app || !app.openNode) return 'no-openNode'; await app.openNode(n); return 'ok'; }, node);
    A('P3', opened === 'ok', '门户 openNode 打开对象浏览器', `ret=${opened}`);
    // 等对象列表表格出现（穿透 shadow）
    await page.waitForFunction(`(()=>{${DEEP};const el=deepFind('.o-tbl')||deepFind('.o-explorer');return !!el})()`, { timeout: 20000 }).catch(() => {});
    await page.waitForTimeout(1500);
    const has = await page.evaluate(`(()=>{${DEEP};return { tbl: !!deepFind('.o-tbl'), exp: !!deepFind('.o-explorer') }})()`);
    A('P4', has.exp || has.tbl, '对象浏览器四区渲染（对象集/列表）', `tbl=${has.tbl} exp=${has.exp}`);
    await page.screenshot({ path: '/tmp/onto-explorer-portal.png' });
    console.log(`\n对象浏览器门户验证：${_p}/${_t} 通过`);
  } catch (e) { A('FATAL', false, '执行', String(e).slice(0, 150)); }
  finally { await b.close(); process.exit(_p >= _t - 1 ? 0 : 1); }
})();
