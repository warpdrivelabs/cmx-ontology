//! 本体建模控制台（O1 建模台雏形）——自包含单页 HTML，挂根路径 `/`。
//!
//! 纯 vanilla JS，调 `/api/onto/v1/*` REST：六类元素统计块 + 对象类型/关系类型列表与新建表单 +
//! 发布/版本。双主题（light/dark，localStorage 持久）。O8 会替换为门户联邦下的完整四区建模工作台。

use axum::response::Html;

/// GET / —— 本体建模控制台。
pub async fn dashboard() -> Html<&'static str> {
    Html(PAGE)
}

const PAGE: &str = r##"<!doctype html>
<html lang="zh-CN" data-theme="dark">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>cmx-ontology · 本体建模控制台</title>
<style>
  :root{
    --bg:#0b1020; --panel:#121a2e; --panel2:#0f1728; --border:#243049; --fg:#e6ecf5;
    --muted:#94a3b8; --accent:#22d3ee; --accent2:#6366f1; --ok:#22c55e; --warn:#f59e0b; --err:#ef4444;
    --chip:#1b2740;
  }
  html[data-theme="light"]{
    --bg:#f6f8fc; --panel:#ffffff; --panel2:#f0f4fa; --border:#e2e8f0; --fg:#0f172a;
    --muted:#64748b; --accent:#0891b2; --accent2:#4f46e5; --chip:#eef2ff;
  }
  *{box-sizing:border-box}
  body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI","PingFang SC","Microsoft YaHei",sans-serif}
  header{display:flex;align-items:center;gap:16px;padding:14px 22px;border-bottom:1px solid var(--border);background:linear-gradient(90deg,var(--panel),var(--panel2));position:sticky;top:0;z-index:5}
  header h1{font-size:16px;margin:0;font-weight:700;letter-spacing:.2px}
  header .sub{color:var(--muted);font-size:12px}
  header .spacer{flex:1}
  button{cursor:pointer;border:1px solid var(--border);background:var(--chip);color:var(--fg);border-radius:8px;padding:7px 12px;font-size:13px;transition:.15s}
  button:hover{border-color:var(--accent)}
  button.primary{background:linear-gradient(90deg,var(--accent2),var(--accent));border:none;color:#fff;font-weight:600}
  button.danger{color:var(--err)}
  main{max-width:1180px;margin:0 auto;padding:22px}
  .tiles{display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:12px;margin-bottom:22px}
  .tile{background:var(--panel);border:1px solid var(--border);border-radius:12px;padding:14px 16px}
  .tile .n{font-size:26px;font-weight:800;letter-spacing:.5px}
  .tile .l{color:var(--muted);font-size:12px;margin-top:2px}
  .tile.accent .n{color:var(--accent)}
  .grid{display:grid;grid-template-columns:1.2fr .8fr;gap:18px;margin-bottom:22px}
  @media(max-width:900px){.grid{grid-template-columns:1fr}}
  .card{background:var(--panel);border:1px solid var(--border);border-radius:12px;overflow:hidden}
  .card h2{font-size:14px;margin:0;padding:12px 16px;border-bottom:1px solid var(--border);background:var(--panel2);display:flex;align-items:center;gap:8px}
  .card h2 .badge{margin-left:auto;color:var(--muted);font-weight:400;font-size:12px}
  .card .body{padding:14px 16px}
  table{width:100%;border-collapse:collapse;font-size:13px}
  th,td{text-align:left;padding:8px 16px;border-bottom:1px solid var(--border)}
  th{color:var(--muted);font-weight:600;font-size:12px}
  tr:last-child td{border-bottom:none}
  .status{font-size:11px;padding:1px 8px;border-radius:20px;border:1px solid var(--border)}
  .status.experimental{color:var(--warn)}
  .status.active{color:var(--ok)}
  .status.deprecated{color:var(--muted)}
  label{display:block;color:var(--muted);font-size:12px;margin:10px 0 4px}
  input,select{width:100%;background:var(--panel2);border:1px solid var(--border);color:var(--fg);border-radius:8px;padding:7px 10px;font-size:13px}
  .row{display:flex;gap:8px}
  .row>*{flex:1}
  .props{margin-top:8px}
  .prop{display:grid;grid-template-columns:1.2fr 1fr auto auto;gap:6px;margin-bottom:6px;align-items:center}
  .prop input[type=checkbox]{width:auto}
  .muted{color:var(--muted)}
  .empty{color:var(--muted);padding:18px 16px;text-align:center;font-size:13px}
  #toast{position:fixed;right:20px;bottom:20px;display:flex;flex-direction:column;gap:8px;z-index:50}
  .t{background:var(--panel);border:1px solid var(--border);border-left:3px solid var(--accent);border-radius:8px;padding:10px 14px;font-size:13px;box-shadow:0 8px 24px rgba(0,0,0,.25);max-width:360px}
  .t.err{border-left-color:var(--err)} .t.ok{border-left-color:var(--ok)}
  code{background:var(--chip);padding:1px 6px;border-radius:6px;font-size:12px}
</style>
</head>
<body>
<header>
  <h1>🕸 cmx-ontology</h1>
  <span class="sub">Palantir 式企业本体平台 · 建模控制台 (O1)</span>
  <span class="spacer"></span>
  <button id="publishBtn" class="primary">📦 发布本体</button>
  <button id="themeBtn">🌓 主题</button>
  <button id="refreshBtn">↻ 刷新</button>
</header>
<main>
  <div class="tiles" id="tiles"></div>

  <div class="grid">
    <div class="card">
      <h2>📦 对象类型 <span class="badge" id="otBadge"></span></h2>
      <div id="otList"></div>
    </div>
    <div class="card">
      <h2>➕ 新建对象类型</h2>
      <div class="body">
        <div class="row">
          <div><label>apiName</label><input id="ot_api" placeholder="Customer"/></div>
          <div><label>显示名</label><input id="ot_disp" placeholder="客户"/></div>
        </div>
        <label>主键属性（须在下方属性中）</label>
        <input id="ot_pk" placeholder="id"/>
        <label>属性 <span class="muted">(apiName / 类型 / 必填 / 删)</span></label>
        <div class="props" id="ot_props"></div>
        <button id="ot_addprop">+ 加属性</button>
        <div style="margin-top:12px"><button class="primary" id="ot_save">保存对象类型</button></div>
      </div>
    </div>
  </div>

  <div class="grid">
    <div class="card">
      <h2>🔗 关系类型 <span class="badge" id="ltBadge"></span></h2>
      <div id="ltList"></div>
    </div>
    <div class="card">
      <h2>➕ 新建关系类型</h2>
      <div class="body">
        <div class="row">
          <div><label>apiName</label><input id="lt_api" placeholder="customerPlacesOrder"/></div>
          <div><label>显示名</label><input id="lt_disp" placeholder="客户下单"/></div>
        </div>
        <div class="row">
          <div><label>A 端对象类型</label><input id="lt_a" placeholder="Customer"/></div>
          <div><label>B 端对象类型</label><input id="lt_b" placeholder="Order"/></div>
        </div>
        <div class="row">
          <div><label>基数</label>
            <select id="lt_card"><option value="oneToMany">oneToMany</option><option value="oneToOne">oneToOne</option><option value="manyToMany">manyToMany</option></select>
          </div>
          <div><label>A→B 角色</label><input id="lt_ra" placeholder="places"/></div>
          <div><label>B→A 角色</label><input id="lt_rb" placeholder="placedBy"/></div>
        </div>
        <div style="margin-top:12px"><button class="primary" id="lt_save">保存关系类型</button></div>
      </div>
    </div>
  </div>

  <div class="card">
    <h2>🏷 发布版本 <span class="badge" id="verBadge"></span></h2>
    <div id="verList"></div>
  </div>
</main>
<div id="toast"></div>

<script>
const API='/api/onto/v1';
const $=id=>document.getElementById(id);
function toast(msg,kind){const t=document.createElement('div');t.className='t '+(kind||'');t.textContent=msg;$('toast').appendChild(t);setTimeout(()=>t.remove(),4200);}
async function api(path,opts){
  const r=await fetch(API+path,Object.assign({headers:{'Content-Type':'application/json'}},opts||{}));
  let j={};try{j=await r.json();}catch(e){}
  if(!r.ok||(j&&j.code&&j.code!==0)){throw new Error((j&&(j.msg||j.message))||('HTTP '+r.status));}
  return j.data;
}
function esc(s){return String(s==null?'':s).replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));}
function statusChip(s){s=s||'experimental';return '<span class="status '+s+'">'+s+'</span>';}

const BASE_TYPES=['string','integer','long','double','decimal','boolean','date','timestamp','array','struct','attachment','mediaReference','marking','geohash','geoShape','vector'];

function addPropRow(api,type,req){
  const wrap=document.createElement('div');wrap.className='prop';
  const opts=BASE_TYPES.map(t=>'<option value="'+t+'"'+(t===type?' selected':'')+'>'+t+'</option>').join('');
  wrap.innerHTML='<input placeholder="apiName" value="'+esc(api||'')+'"/>'+
    '<select>'+opts+'</select>'+
    '<label class="muted" style="margin:0;display:flex;align-items:center;gap:4px"><input type="checkbox"'+(req?' checked':'')+'/>必填</label>'+
    '<button class="danger" title="删除">✕</button>';
  wrap.querySelector('button').onclick=()=>wrap.remove();
  $('ot_props').appendChild(wrap);
}
function collectProps(){
  return [...$('ot_props').children].map(w=>{
    const[i,s]=w.querySelectorAll('input,select');
    const cb=w.querySelector('input[type=checkbox]');
    return{apiName:i.value.trim(),baseType:s.value,required:cb.checked};
  }).filter(p=>p.apiName);
}

async function loadStats(){
  const s=await api('/stats');
  const tiles=[
    ['对象类型',s.objectTypes,'accent'],['关系类型',s.linkTypes,''],['接口',s.interfaces,''],
    ['共享属性',s.sharedProperties,''],['动作类型',s.actionTypes,''],['函数',s.functions,''],
    ['已发布版本',s.publishedVersion,'accent'],
  ];
  $('tiles').innerHTML=tiles.map(t=>'<div class="tile '+t[2]+'"><div class="n">'+t[1]+'</div><div class="l">'+t[0]+'</div></div>').join('');
}
async function loadObjectTypes(){
  const list=await api('/object-types');
  $('otBadge').textContent=list.length+' 个';
  if(!list.length){$('otList').innerHTML='<div class="empty">暂无对象类型，右侧新建一个 →</div>';return;}
  $('otList').innerHTML='<table><thead><tr><th>apiName</th><th>显示名</th><th>主键</th><th>属性</th><th>状态</th><th></th></tr></thead><tbody>'+
    list.map(o=>'<tr><td><code>'+esc(o.apiName)+'</code></td><td>'+esc(o.displayName)+'</td><td>'+esc(o.primaryKey)+'</td><td>'+o.propertyCount+'</td><td>'+statusChip(o.status)+'</td>'+
      '<td><button class="danger" data-del="'+esc(o.apiName)+'">删</button></td></tr>').join('')+'</tbody></table>';
  $('otList').querySelectorAll('button[data-del]').forEach(b=>b.onclick=async()=>{
    if(!confirm('删除对象类型 '+b.dataset.del+' ?'))return;
    try{await api('/object-types/'+encodeURIComponent(b.dataset.del),{method:'DELETE'});toast('已删除 '+b.dataset.del,'ok');refresh();}catch(e){toast(e.message,'err');}
  });
}
async function loadLinkTypes(){
  const list=await api('/link-types');
  $('ltBadge').textContent=list.length+' 个';
  if(!list.length){$('ltList').innerHTML='<div class="empty">暂无关系类型</div>';return;}
  $('ltList').innerHTML='<table><thead><tr><th>apiName</th><th>A 端</th><th>基数</th><th>B 端</th><th></th></tr></thead><tbody>'+
    list.map(l=>'<tr><td><code>'+esc(l.apiName)+'</code></td><td>'+esc(l.objectTypeA)+'</td><td class="muted">'+esc(l.cardinality)+'</td><td>'+esc(l.objectTypeB)+'</td>'+
      '<td><button class="danger" data-del="'+esc(l.apiName)+'">删</button></td></tr>').join('')+'</tbody></table>';
  $('ltList').querySelectorAll('button[data-del]').forEach(b=>b.onclick=async()=>{
    try{await api('/link-types/'+encodeURIComponent(b.dataset.del),{method:'DELETE'});toast('已删除 '+b.dataset.del,'ok');refresh();}catch(e){toast(e.message,'err');}
  });
}
async function loadVersions(){
  const list=await api('/versions');
  $('verBadge').textContent=list.length+' 个版本';
  if(!list.length){$('verList').innerHTML='<div class="empty">尚未发布。点右上「发布本体」生成不可变快照。</div>';return;}
  $('verList').innerHTML='<table><thead><tr><th>版本</th><th>rev</th><th>摘要</th><th>发布人</th><th>时间</th></tr></thead><tbody>'+
    list.map(v=>'<tr><td>v'+v.version+'</td><td><code>'+esc(v.rev)+'</code></td><td>'+esc(v.summary)+'</td><td>'+esc(v.publishedBy||'-')+'</td><td class="muted">'+esc((v.publishedAt||'').replace('T',' ').slice(0,19))+'</td></tr>').join('')+'</tbody></table>';
}

async function refresh(){try{await Promise.all([loadStats(),loadObjectTypes(),loadLinkTypes(),loadVersions()]);}catch(e){toast('加载失败: '+e.message,'err');}}

$('ot_addprop').onclick=()=>addPropRow('','string',false);
$('ot_save').onclick=async()=>{
  const def={apiName:$('ot_api').value.trim(),displayName:$('ot_disp').value.trim(),primaryKey:$('ot_pk').value.trim(),properties:collectProps()};
  if(!def.apiName){toast('apiName 必填','err');return;}
  try{await api('/object-types',{method:'POST',body:JSON.stringify(def)});toast('已保存 '+def.apiName,'ok');
    $('ot_api').value='';$('ot_disp').value='';$('ot_pk').value='';$('ot_props').innerHTML='';addPropRow('id','string',true);refresh();
  }catch(e){toast(e.message,'err');}
};
$('lt_save').onclick=async()=>{
  const def={apiName:$('lt_api').value.trim(),displayName:$('lt_disp').value.trim(),objectTypeA:$('lt_a').value.trim(),objectTypeB:$('lt_b').value.trim(),cardinality:$('lt_card').value,roleA:$('lt_ra').value.trim(),roleB:$('lt_rb').value.trim()};
  if(!def.apiName||!def.objectTypeA||!def.objectTypeB){toast('apiName / A 端 / B 端 必填','err');return;}
  try{await api('/link-types',{method:'POST',body:JSON.stringify(def)});toast('已保存 '+def.apiName,'ok');
    $('lt_api').value='';$('lt_disp').value='';$('lt_a').value='';$('lt_b').value='';$('lt_ra').value='';$('lt_rb').value='';refresh();
  }catch(e){toast(e.message,'err');}
};
$('publishBtn').onclick=async()=>{
  const summary=prompt('发布摘要（本次发布的说明）:','');
  if(summary===null)return;
  try{const m=await api('/publish',{method:'POST',body:JSON.stringify({summary})});toast('已发布 v'+m.version+' (rev '+m.rev+')','ok');refresh();}catch(e){toast(e.message,'err');}
};
$('refreshBtn').onclick=refresh;
$('themeBtn').onclick=()=>{const h=document.documentElement;const t=h.getAttribute('data-theme')==='dark'?'light':'dark';h.setAttribute('data-theme',t);localStorage.setItem('onto-theme',t);};
(function init(){const t=localStorage.getItem('onto-theme');if(t)document.documentElement.setAttribute('data-theme',t);addPropRow('id','string',true);refresh();})();
</script>
</body>
</html>"##;
