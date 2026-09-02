/* 生成《大模型 + 智能体 + 本体：企业智能型系统建设方案》——图文并茂 markdown。
 * 本脚本内置 SVG 生成器 + 全部图元 + 正文，产出内嵌 base64 SVG(<img>)的单文件 md。
 * 运行：node docs/assets/organism/build.cjs   → docs/20260901_大模型+智能体+本体_企业智能系统建设方案.md
 */
'use strict';
const fs = require('fs');
const path = require('path');

const FONT = "'Inter','Segoe UI','PingFang SC','Microsoft YaHei',system-ui,sans-serif";
const C = {
  bg0: '#070b16', bg1: '#0b1120', card: '#0e1526', card2: '#121b30', stroke: '#1c2740', stroke2: '#2b3a5c',
  fg: '#e8eefc', mut: '#9fb0cc', dim: '#5f7290',
  llm: '#a78bfa', llmD: '#6d28d9', agent: '#22d3ee', agentD: '#0e7490',
  onto: '#34d399', ontoD: '#047857', sense: '#fbbf24', senseD: '#b45309',
  act: '#fb7185', actD: '#be123c', nerve: '#818cf8', nerveD: '#4338ca',
  ent: '#94a3b8', erp: '#f59e0b', crm: '#38bdf8', oa: '#a3e635', flow: '#c084fc', rpt: '#4ade80', db: '#64748b',
  ok: '#34d399', warn: '#fbbf24', bad: '#fb7185',
};
const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

// ── SVG 基元 ──
function T(x, y, s, o = {}) {
  const { size = 15, fill = C.fg, anchor = 'start', w = 400, ls = 0, op = 1 } = o;
  return `<text x="${x}" y="${y}" font-size="${size}" fill="${fill}" text-anchor="${anchor}" font-weight="${w}" letter-spacing="${ls}" opacity="${op}">${esc(s)}</text>`;
}
function TM(x, y, lines, o = {}) { // 多行
  const { lh = 18 } = o;
  return lines.map((l, i) => T(x, y + i * lh, l, o)).join('');
}
function R(x, y, w, h, o = {}) {
  const { r = 12, fill = 'none', stroke = 'none', sw = 1, op = 1, dash = '' } = o;
  return `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${r}" fill="${fill}" stroke="${stroke}" stroke-width="${sw}" opacity="${op}"${dash ? ` stroke-dasharray="${dash}"` : ''}/>`;
}
function C_(cx, cy, r, o = {}) {
  const { fill = 'none', stroke = 'none', sw = 1, op = 1 } = o;
  return `<circle cx="${cx}" cy="${cy}" r="${r}" fill="${fill}" stroke="${stroke}" stroke-width="${sw}" opacity="${op}"/>`;
}
function L(x1, y1, x2, y2, o = {}) {
  const { stroke = C.stroke2, sw = 1.4, dash = '', op = 1, cap = 'round', mk = '' } = o;
  return `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${stroke}" stroke-width="${sw}" opacity="${op}" stroke-linecap="${cap}"${dash ? ` stroke-dasharray="${dash}"` : ''}${mk ? ` marker-end="url(#${mk})"` : ''}/>`;
}
function P(d, o = {}) {
  const { stroke = 'none', sw = 1.6, fill = 'none', dash = '', op = 1, cap = 'round', mk = '' } = o;
  return `<path d="${d}" stroke="${stroke}" stroke-width="${sw}" fill="${fill}" opacity="${op}" stroke-linecap="${cap}" stroke-linejoin="round"${dash ? ` stroke-dasharray="${dash}"` : ''}${mk ? ` marker-end="url(#${mk})"` : ''}/>`;
}
// 圆角标签 chip：icon 圆点 + 文本
function chip(x, y, w, h, label, col, o = {}) {
  const { sub = '', r = 11, fill = C.card2, tsize = 14 } = o;
  let s = R(x, y, w, h, { r, fill, stroke: col, sw: 1.3, op: 1 });
  s += C_(x + 16, y + h / 2, 4.5, { fill: col });
  s += T(x + 30, y + (sub ? h / 2 - 2 : h / 2 + 5), label, { size: tsize, fill: C.fg, w: 600 });
  if (sub) s += T(x + 30, y + h / 2 + 14, sub, { size: 11.5, fill: C.mut });
  return s;
}
function badge(x, y, txt, col) {
  const w = 20 + txt.length * 8.6;
  return R(x, y, w, 24, { r: 12, fill: col + '22', stroke: col, sw: 1 }) + T(x + w / 2, y + 16, txt, { size: 12.5, anchor: 'middle', fill: col, w: 600 });
}
function markers() {
  const mk = (id, col) => `<marker id="${id}" markerWidth="9" markerHeight="9" refX="7" refY="4" orient="auto"><path d="M0.5,0.5 L8,4 L0.5,7.5 L3,4 Z" fill="${col}"/></marker>`;
  return mk('aFg', C.mut) + mk('aSense', C.sense) + mk('aAct', C.act) + mk('aNerve', C.nerve) + mk('aLlm', C.llm) + mk('aAgent', C.agent) + mk('aOnto', C.onto) + mk('aOk', C.ok) + mk('aEnt', C.ent);
}
function grad(id, c1, c2, vert = true) {
  return `<linearGradient id="${id}" x1="0" y1="0" x2="${vert ? 0 : 1}" y2="${vert ? 1 : 0}"><stop offset="0" stop-color="${c1}"/><stop offset="1" stop-color="${c2}"/></linearGradient>`;
}
function DEFS(w, h) {
  return `<defs>
    <linearGradient id="bgG" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${C.bg1}"/><stop offset="1" stop-color="${C.bg0}"/></linearGradient>
    ${grad('llmG', '#c4b5fd', C.llmD)}${grad('agentG', '#67e8f9', C.agentD)}${grad('ontoG', '#6ee7b7', C.ontoD)}
    ${grad('senseG', '#fcd34d', C.senseD)}${grad('actG', '#fda4af', C.actD)}${grad('nerveG', '#a5b4fc', C.nerveD)}
    <radialGradient id="glowLlm" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="${C.llm}" stop-opacity="0.55"/><stop offset="1" stop-color="${C.llm}" stop-opacity="0"/></radialGradient>
    <radialGradient id="glowOnto" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="${C.onto}" stop-opacity="0.4"/><stop offset="1" stop-color="${C.onto}" stop-opacity="0"/></radialGradient>
    <filter id="soft" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
    ${markers()}
  </defs>`;
}
function svg(w, h, inner) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" font-family="${FONT}">${DEFS(w, h)}<rect x="1.5" y="1.5" width="${w - 3}" height="${h - 3}" rx="24" fill="url(#bgG)" stroke="#18223c"/>${inner}</svg>`;
}
function title(x, y, t, sub) {
  return T(x, y, t, { size: 27, w: 800, fill: C.fg, ls: 0.3 }) + (sub ? T(x, y + 26, sub, { size: 14.5, fill: C.mut }) : '');
}

// ═══════════ 图 1：数字生命体总览 ═══════════
function heroOrganism() {
  const W = 1200, H = 840; let s = '';
  s += title(48, 62, '数字生命体 · The Cognitive Enterprise Organism', '大模型(大脑) · 智能体(小脑) · 本体(肌体/感官/神经) —— 一个能思考、会指挥、能动手的企业智能有机体');
  const cx = 470;
  // 光晕
  s += C_(cx, 190, 150, { fill: 'url(#glowLlm)' });
  s += C_(cx, 470, 170, { fill: 'url(#glowOnto)' });
  // 大脑 LLM
  s += `<path d="M${cx - 92},175 q-30,-58 30,-74 q26,-40 78,-18 q44,-22 70,20 q52,16 26,66 q22,44 -26,62 q-30,40 -84,20 q-46,26 -84,-12 q-46,-16 -6,-64 Z" fill="url(#llmG)" stroke="${C.llm}" stroke-width="1.5" filter="url(#soft)"/>`;
  // 脑沟
  s += P(`M${cx - 40},150 q20,-14 40,0 q20,14 40,0`, { stroke: '#3b1e70', sw: 2, op: .6 });
  s += P(`M${cx - 46},178 q26,-16 46,0 q22,16 48,-2`, { stroke: '#3b1e70', sw: 2, op: .6 });
  s += T(cx, 168, '大模型 LLM', { size: 17, anchor: 'middle', w: 800, fill: '#1e1043' });
  s += T(cx, 190, '思考 · 推理 · 规划', { size: 11.5, anchor: 'middle', fill: '#2a1755' });
  // 小脑 Agent
  s += `<ellipse cx="${cx}" cy="285" rx="86" ry="40" fill="url(#agentG)" stroke="${C.agent}" stroke-width="1.4"/>`;
  s += P(`M${cx - 60},285 q30,-16 60,0 q30,16 60,0`, { stroke: '#083b47', sw: 1.6, op: .5 });
  s += P(`M${cx - 60},296 q30,-16 60,0 q30,16 60,0`, { stroke: '#083b47', sw: 1.6, op: .5 });
  s += T(cx, 282, '智能体 Agent', { size: 15.5, anchor: 'middle', w: 800, fill: '#04303a' });
  s += T(cx, 300, '编排 · 决策 · 指挥行动', { size: 11, anchor: 'middle', fill: '#06414f' });
  // 脊髓 神经系统
  s += L(cx, 325, cx, 640, { stroke: C.nerve, sw: 6, dash: '2 9', op: .9 });
  [370, 430, 490, 550, 610].forEach(y => { s += C_(cx, y, 5.5, { fill: C.nerveD, stroke: C.nerve, sw: 1.2 }); });
  s += T(cx + 16, 372, '神经系统', { size: 12.5, fill: C.nerve, w: 700 });
  s += T(cx + 16, 388, '语义本体 + 事件总线 + Outbox', { size: 10.5, fill: C.mut });
  // 躯干 本体
  s += `<path d="M${cx - 108},372 q108,-30 216,0 l-22,196 q-86,26 -172,0 Z" fill="${C.onto}18" stroke="${C.onto}" stroke-width="1.7"/>`;
  s += T(cx, 470, '本 体', { size: 22, anchor: 'middle', w: 800, fill: C.onto });
  s += T(cx, 494, '语义模型 · 数字肌体', { size: 12.5, anchor: 'middle', fill: C.mut });
  s += T(cx, 512, '对象 / 关系 / 动作 / 函数', { size: 11, anchor: 'middle', fill: C.dim });
  // 左臂 → 感知
  s += P(`M${cx - 96},392 Q${cx - 200},420 ${cx - 250},452`, { stroke: C.sense, sw: 9 });
  s += C_(cx - 258, 456, 15, { fill: C.senseD, stroke: C.sense, sw: 1.6 });
  // 右臂 → 行动
  s += P(`M${cx + 96},392 Q${cx + 210},420 ${cx + 270},452`, { stroke: C.act, sw: 9 });
  s += C_(cx + 278, 456, 15, { fill: C.actD, stroke: C.act, sw: 1.6 });
  // 腿 基础设施
  s += P(`M${cx - 44},564 Q${cx - 66},650 ${cx - 78},716`, { stroke: '#3a4a66', sw: 11 });
  s += P(`M${cx + 44},564 Q${cx + 66},650 ${cx + 78},716`, { stroke: '#3a4a66', sw: 11 });
  s += R(cx - 150, 726, 300, 40, { r: 12, fill: C.card2, stroke: C.stroke2 });
  s += T(cx, 751, '基础设施底座 · 多租户 · 数据底座 · 可观测', { size: 12.5, anchor: 'middle', fill: C.mut, w: 600 });
  // 感官（头部小图标）
  s += C_(cx - 30, 205, 5, { fill: C.sense }); s += C_(cx + 30, 205, 5, { fill: C.sense });
  s += `<path d="M${cx - 96},188 q-14,4 -10,22" stroke="${C.sense}" stroke-width="2.4" fill="none"/><path d="M${cx + 96},188 q14,4 10,22" stroke="${C.sense}" stroke-width="2.4" fill="none"/>`;
  // 左：感知源
  const senseSrc = [['业务数据库', 'ERP/CRM 表'], ['API / 消息', 'REST · MQ · 事件'], ['文档 / 文件', 'PDF · Excel'], ['IoT / 埋点', '设备 · 行为流']];
  senseSrc.forEach((it, i) => { const y = 372 + i * 58; s += chip(60, y, 168, 46, it[0], C.sense, { sub: it[1] }); s += P(`M228,${y + 23} Q${cx - 300},${y + 23} ${cx - 272},456`, { stroke: C.sense, sw: 1.6, dash: '5 5', op: .8, mk: 'aSense' }); });
  s += T(60, 358, '感知系统 · 感官(耳/眼/口/舌)', { size: 13, fill: C.sense, w: 700 });
  // 右：企业系统
  const entSys = [['ERP 系统', '过账 · 更新单据', C.erp], ['流程引擎 flow', '发起 · 审批', C.flow], ['报表平台 report', '计算 · 出表', C.rpt], ['CRM / OA / MES', '写回 · 通知', C.crm], ['外部 Webhook', '触达三方', C.act]];
  entSys.forEach((it, i) => { const y = 356 + i * 52; s += chip(cx + 320, y, 214, 42, it[0], it[2], { sub: it[1] }); s += P(`M${cx + 292},456 Q${cx + 300},${y + 21} ${cx + 320},${y + 21}`, { stroke: C.act, sw: 1.7, mk: 'aAct', op: .9 }); });
  s += T(cx + 320, 342, '行动系统 · 肢体(手/脚) → 操作企业现有系统', { size: 13, fill: C.act, w: 700 });
  // 反馈大弧：企业 → 大脑
  s += P(`M${cx + 534},372 Q${cx + 660},128 ${cx + 40},112 Q${cx - 40},110 ${cx - 30},128`, { stroke: C.ok, sw: 2, dash: '7 6', op: .85, mk: 'aOk' });
  s += T(cx + 372, 100, '反馈回流：结果回喂大脑 · 闭环学习', { size: 12.5, fill: C.ok, w: 600 });
  return svg(W, H, s);
}

// ═══════════ 图 2：四层架构 ═══════════
function layeredArch() {
  const W = 1200, H = 720; let s = '';
  s += title(48, 60, '总体架构 · 四层解剖', '思考层(LLM) → 决策层(Agent) → 肌体层(本体) → 执行对象层(企业系统)，纵向一条感知-行动神经贯通');
  const layers = [
    ['① 思考层 · 大脑', 'LLM', C.llm, 'url(#llmG)', ['自然语言理解意图 / 多轮对话 / 任务拆解', '基于本体上下文的推理与规划(RAG/工具调用)', '生成结构化「行动意图」交给 Agent', '记忆：短时(会话) + 长时(向量/图谱)']],
    ['② 决策层 · 小脑', 'Agent', C.agent, 'url(#agentG)', ['把意图编排为动作序列(ReAct / Plan-Execute)', '工具选择 = 选本体「动作 / 函数 / 对象集」', '人在环(HITL)审批闸门 · 失败重试 · 补偿', '并发/依赖调度 · 预演(dry-run)后提交']],
    ['③ 肌体层 · 本体', 'Ontology', C.onto, 'url(#ontoG)', ['语义模型：对象/关系/接口/共享属性(神经)', '感知：数据集成/对象集/函数/搜索(感官)', '行动：动作引擎 + 副作用 dispatcher(肌肉)', '治理：数据权限 PDP/PEP · 审计 · Outbox']],
    ['④ 执行对象层 · 企业系统', 'Systems', C.ent, 'none', ['ERP / CRM / OA / MES / 主数据', '流程引擎 · 报表平台 · 规则引擎', '数据库 · 消息队列 · 三方 Webhook', '文件/文档/IoT —— 既是感知源也是行动对象']],
  ];
  let y = 96;
  layers.forEach((ly, i) => {
    const h = 118; const fill = ly[3] === 'none' ? C.card2 : ly[3];
    s += R(48, y, 300, h, { r: 16, fill, stroke: ly[2], sw: 1.5 });
    s += T(72, y + 40, ly[0], { size: 18, w: 800, fill: ly[3] === 'none' ? C.fg : '#0b1120' });
    s += badge(72, y + 58, ly[1], ly[3] === 'none' ? C.ent : '#0b1120');
    // 右侧职责卡
    s += R(372, y, 780, h, { r: 16, fill: C.card, stroke: C.stroke });
    ly[4].forEach((d, j) => { const col = j % 2; const cxp = 396 + col * 388, cyp = y + 34 + Math.floor(j / 2) * 42; s += C_(cxp + 3, cyp - 4, 3.5, { fill: ly[2] }); s += T(cxp + 16, cyp, d, { size: 13.5, fill: C.mut }); });
    if (i < layers.length - 1) { s += L(198, y + h, 198, y + h + 24, { stroke: C.nerve, sw: 4, dash: '2 6', mk: 'aNerve' }); }
    y += h + 24;
  });
  return svg(W, H, s);
}

// ═══════════ 图 3：感知-认知-决策-行动-反馈 闭环 ═══════════
function loopCycle() {
  const W = 1200, H = 720; let s = '';
  s += title(48, 60, '认知闭环 · Sense → Think → Decide → Act → Learn', '企业智能体的 OODA 循环：一切从感知开始，以行动落地，用反馈进化');
  const cx = 600, cy = 400, R0 = 230;
  s += C_(cx, cy, R0, { stroke: C.stroke2, sw: 1.2, dash: '3 8', op: .6 });
  const nodes = [
    ['感 知', 'Sense', '本体读企业数据\n对象集/函数/搜索', C.sense, -90],
    ['认 知', 'Think', 'LLM 理解 + 推理\n结合本体上下文', C.llm, -18],
    ['决 策', 'Decide', 'Agent 编排动作\nHITL 审批闸门', C.agent, 54],
    ['行 动', 'Act', '动作引擎写回\n操作 ERP/流程/报表', C.act, 126],
    ['学 习', 'Learn', '结果回流大脑\n沉淀记忆/优化', C.ok, 198],
  ];
  nodes.forEach((n, i) => {
    const a = n[4] * Math.PI / 180, x = cx + R0 * Math.cos(a), y = cy + R0 * Math.sin(a);
    // 连接弧箭头到下一个
    const a2 = nodes[(i + 1) % 5][4] * Math.PI / 180;
    const mx = cx + (R0 + 0) * Math.cos((a + a2) / 2 + (a2 < a ? Math.PI : 0)), my = cy + (R0) * Math.sin((a + a2) / 2 + (a2 < a ? Math.PI : 0));
    const x2 = cx + R0 * Math.cos(a2), y2 = cy + R0 * Math.sin(a2);
    s += P(`M${x},${y} A${R0},${R0} 0 0 1 ${x2},${y2}`, { stroke: n[3], sw: 2.4, op: .8, mk: 'a' + ['Sense', 'Llm', 'Agent', 'Act', 'Ok'][i] });
    s += C_(x, y, 62, { fill: C.card2, stroke: n[3], sw: 2 });
    s += T(x, y - 6, n[0], { size: 17, anchor: 'middle', w: 800, fill: n[3] });
    s += T(x, y + 12, n[1], { size: 11, anchor: 'middle', fill: C.mut });
    n[2].split('\n').forEach((ln, k) => s += T(x, y + 84 + k * 15, ln, { size: 11.5, anchor: 'middle', fill: C.mut }));
  });
  // 中心
  s += C_(cx, cy, 74, { fill: C.card, stroke: C.onto, sw: 1.6 });
  s += T(cx, cy - 8, '本 体', { size: 18, anchor: 'middle', w: 800, fill: C.onto });
  s += T(cx, cy + 12, '闭环中枢', { size: 11.5, anchor: 'middle', fill: C.mut });
  s += T(cx, cy + 30, '语义 + 神经', { size: 10.5, anchor: 'middle', fill: C.dim });
  return svg(W, H, s);
}

// ═══════════ 图 4：本体解剖 = 神经-肌肉系统 ═══════════
function ontologyAnatomy() {
  const W = 1200, H = 760; let s = '';
  s += title(48, 60, '本体解剖 · 语义即神经，动作即肌肉', '把 Palantir 式本体的每一类元素，映射为生命体的一个器官/系统');
  const rows = [
    ['对象类型 Object Type', '器官 / 细胞', '业务实体的强类型定义(订单/客户/凭证)——身体的组成单元', C.onto, 'oo_*'],
    ['关系 Link Type', '神经连接', '对象之间的语义连线，信号沿其传导(下单→客户)', C.nerve, 'ol_edge'],
    ['接口 Interface', '组织/系统', '跨对象的共性契约，如同"循环系统"横切多器官', C.agent, 'om_interface'],
    ['共享属性 Shared Prop', '基因', '可复用的属性基元，保证同一语义处处一致', C.crm, 'om_shared'],
    ['函数 Function(O5)', '反射 / 感官', 'FEEL/脚本计算——查询、派生、聚合，读世界并回传', C.sense, 'om_function'],
    ['动作 Action(O4)', '肌肉 / 肢体', '受治理的写入口：改对象 + 触发副作用，身体的"动手"', C.act, 'om_action'],
    ['副作用 Side-Effect', '肌腱 / 末梢', '起流程/算报表/webhook/通知——力达企业外部系统', C.act, 'oe_outbox'],
    ['事件 / Outbox', '神经递质', '事务性投递 + 事件流，把动作与感知的信号可靠送达', C.nerve, 'events/SSE'],
    ['数据集成 Funnel(O3)', '消化 / 摄入', '把外部数据映射进本体(隔离区校验)——把食物变成养分', C.sense, 'om_source'],
    ['数据权限 PDP/PEP', '免疫系统', '决策/执行分离 + 残差约束，抵御越权(本次已为 report 补齐鉴权)', C.warn, 'data-auth'],
  ];
  let y = 96; const rh = 60;
  rows.forEach((r, i) => {
    s += R(48, y, 1104, rh - 8, { r: 12, fill: i % 2 ? C.card : C.card2, stroke: C.stroke });
    s += C_(70, y + 26, 6, { fill: r[3] });
    s += T(92, y + 23, r[0], { size: 14.5, w: 700, fill: C.fg });
    s += T(92, y + 41, r[4], { size: 11, fill: C.dim });
    s += T(350, y + 32, r[1], { size: 14, w: 700, fill: r[3] });
    s += T(520, y + 32, r[2], { size: 12.8, fill: C.mut });
    y += rh;
  });
  return svg(W, H, s);
}

// ═══════════ 图 5：行动系统 → 企业系统集成 ═══════════
function actionIntegration() {
  const W = 1200, H = 640; let s = '';
  s += title(48, 60, '行动系统 · 一个动作，多路副作用触达企业系统', '本体动作引擎(O4) → 事务性 Outbox → dispatcher 分派器 → 各企业系统连接器(真机已打通)');
  // 左：动作
  s += R(48, 130, 250, 220, { r: 16, fill: 'url(#actG)', stroke: C.act, sw: 1.4 });
  s += T(173, 168, '动作引擎 O4', { size: 17, anchor: 'middle', w: 800, fill: '#3d0716' });
  s += T(173, 190, 'Action Engine', { size: 11.5, anchor: 'middle', fill: '#5c0c22' });
  ['① 校验参数 + FEEL 门', '② 解析编辑(改对象)', '③ 单事务写回 + 审计', '④ 副作用入 Outbox'].forEach((t, i) => s += T(72, 222 + i * 28, t, { size: 12.5, fill: '#3d0716', w: 600 }));
  // 中：dispatcher
  s += L(298, 240, 360, 240, { stroke: C.act, sw: 2, mk: 'aAct' });
  s += R(360, 150, 200, 180, { r: 14, fill: C.card2, stroke: C.nerve, sw: 1.4 });
  s += T(460, 184, 'Dispatcher', { size: 16, anchor: 'middle', w: 800, fill: C.nerve });
  s += T(460, 204, '事务性投递 · SKIP LOCKED', { size: 10.8, anchor: 'middle', fill: C.mut });
  ['服务身份 X-API-Key', 'SSRF 白名单护栏', '失败→failed / 熄火→deferred', '错误消息如实回传'].forEach((t, i) => s += T(378, 230 + i * 22, '· ' + t, { size: 11.5, fill: C.mut }));
  // 右：连接器 → 系统
  const outs = [
    ['startBusinessProcess', '→ flow 起流程实例', C.flow, '真机✓'],
    ['computeReport', '→ report 真算落库', C.rpt, '真机✓'],
    ['webhook', '→ 三方系统 HTTP', C.act, '真机✓'],
    ['modifyObject', '→ 本体对象写回', C.onto, '真机✓'],
    ['notification', '→ 通知 / 待办', C.sense, '真机✓'],
    ['(connector)', '→ ERP 过账 / 主数据', C.erp, '规划'],
  ];
  outs.forEach((o, i) => {
    const y = 138 + i * 76; s += L(560, 240, 636, y + 26, { stroke: o[2], sw: 1.6, mk: 'aEnt', op: .85 });
    s += R(636, y, 500, 58, { r: 12, fill: C.card, stroke: o[2], sw: 1.3 });
    s += C_(660, y + 29, 5, { fill: o[2] });
    s += T(680, y + 25, o[0], { size: 13.5, w: 700, fill: C.fg });
    s += T(680, y + 43, o[1], { size: 12, fill: C.mut });
    s += (o[3] === '真机✓' ? badge(1050, y + 16, '真机✓', C.ok) : badge(1058, y + 16, '规划', C.dim));
  });
  return svg(W, H, s);
}

// ═══════════ 图 6：端到端场景时序（月末关账）═══════════
function scenarioSeq() {
  const W = 1200, H = 720; let s = '';
  s += title(48, 58, '端到端场景 · "帮我把 3 月关账"', '一句自然语言，穿过大脑→小脑→本体→企业系统，再把结果回流——全链路真机能力已具备');
  const lanes = [['用户', C.fg], ['大脑 LLM', C.llm], ['小脑 Agent', C.agent], ['本体 Ontology', C.onto], ['企业系统', C.ent]];
  const lx = [120, 340, 560, 790, 1040];
  lanes.forEach((l, i) => { s += R(lx[i] - 78, 92, 156, 34, { r: 10, fill: C.card2, stroke: l[1], sw: 1.3 }); s += T(lx[i], 114, l[0], { size: 13.5, anchor: 'middle', w: 700, fill: l[1] }); s += L(lx[i], 130, lx[i], 660, { stroke: C.stroke2, sw: 1, dash: '3 7' }); });
  const steps = [
    [0, 1, '"帮我把 3 月关账并出资产负债表"', C.fg],
    [1, 3, '① 读本体：查关账动作/报表/组织期间(感知)', C.llm],
    [3, 1, '返回可用「动作模板 consolClose」+ 参数形状', C.onto],
    [1, 2, '② 生成行动意图：org=CSCEC, period=2025-03', C.llm],
    [2, 2, '③ 编排：dry-run 预演 → 呈现给用户确认(HITL)', C.agent],
    [2, 3, '④ execute 关账联动动作', C.agent],
    [3, 4, '⑤ startBusinessProcess → 起关账审批流', C.act],
    [3, 4, '⑥ computeReport → 真算资产负债表落库', C.act],
    [4, 3, '实例已建 + cr_cell_data 已写(结果)', C.ent],
    [3, 1, '⑦ 事件/结果回流(反馈)', C.ok],
    [1, 0, '"已发起关账审批(实例#c5a8)，BS 已生成，待复核"', C.ok],
  ];
  let y = 168;
  steps.forEach((st) => {
    const x1 = lx[st[0]], x2 = lx[st[1]]; const self = st[0] === st[1];
    if (self) { s += P(`M${x1},${y - 6} q70,10 0,26`, { stroke: st[3], sw: 1.8, mk: 'a' + (st[3] === C.agent ? 'Agent' : 'Ok') }); s += T(x1 + 84, y + 6, st[2], { size: 12.2, fill: C.mut }); }
    else {
      const dir = x2 > x1 ? 1 : -1; s += L(x1 + 6 * dir, y, x2 - 8 * dir, y, { stroke: st[3], sw: 1.9, mk: st[3] === C.ok ? 'aOk' : (dir > 0 ? 'aAct' : 'aFg') });
      s += T((x1 + x2) / 2, y - 8, st[2], { size: 12.2, anchor: 'middle', fill: st[3] === C.ok ? C.ok : C.mut, w: st[3] === C.ok ? 600 : 400 });
    }
    y += self ? 44 : 44;
  });
  return svg(W, H, s);
}

// ═══════════ 图 7：建设路线图 ═══════════
function roadmap() {
  const W = 1200, H = 560; let s = '';
  s += title(48, 60, '分阶段建设路线图 · Crawl → Walk → Run', '先把"肌体"立住(已完成大半)，再接"大脑/小脑"，最后进化为自主有机体');
  const phases = [
    ['P0 肌体', '本体平台', ['对象/关系/动作/函数', '数据集成 + 权限', '双服务联动(flow/report)'], C.onto, '已完成'],
    ['P1 神经', '连接与感知', ['ERP/CRM 连接器', '事件总线 + 反馈回流', '本体只读 MCP/工具封装'], C.nerve, '进行中'],
    ['P2 小脑', 'Agent 编排', ['工具=本体动作/函数', 'ReAct + HITL 审批', 'dry-run→提交 + 补偿'], C.agent, '规划'],
    ['P3 大脑', 'LLM 接入', ['NL→意图→动作', '本体 RAG 上下文', '记忆(向量+图谱)'], C.llm, '规划'],
    ['P4 有机体', '自主进化', ['结果回流学习', '多智能体协作', '自愈 + 主动洞察'], C.ok, '愿景'],
  ];
  const w = 210, gap = 15, x0 = 48, y = 120;
  phases.forEach((p, i) => {
    const x = x0 + i * (w + gap);
    s += R(x, y, w, 320, { r: 16, fill: C.card, stroke: p[3], sw: 1.5 });
    s += R(x, y, w, 66, { r: 16, fill: p[3] + '1e' });
    s += T(x + 18, y + 30, p[0], { size: 16, w: 800, fill: p[3] });
    s += T(x + 18, y + 52, p[1], { size: 12.5, fill: C.mut });
    p[2].forEach((d, j) => { s += C_(x + 21, y + 96 + j * 40 - 4, 3.5, { fill: p[3] }); s += TM(x + 34, y + 100 + j * 40, [d], { size: 12, fill: C.mut, lh: 15 }); });
    s += badge(x + 18, y + 274, p[4], p[4] === '已完成' ? C.ok : p[4] === '进行中' ? C.warn : C.dim);
    if (i < phases.length - 1) s += L(x + w, y + 160, x + w + gap, y + 160, { stroke: C.stroke2, sw: 2, mk: 'aFg' });
  });
  return svg(W, H, s);
}

// ── 组装 ──
const D = { hero: heroOrganism(), arch: layeredArch(), loop: loopCycle(), anatomy: ontologyAnatomy(), action: actionIntegration(), scenario: scenarioSeq(), roadmap: roadmap() };
function img(k, alt) { const b = Buffer.from(D[k]).toString('base64'); return `<div align="center"><img alt="${esc(alt)}" width="1040" src="data:image/svg+xml;base64,${b}"/></div>`; }

const md = `# 大模型 + 智能体 + 本体：企业智能型系统建设方案

> **一句话立意**：把企业智能系统建成一个**数字生命体**——**大模型(LLM)是大脑负责思考，智能体(Agent)是小脑负责指挥行动，本体(Ontology)是肌体(四肢)、感官(耳眼口舌)与神经系统**。感知与行动的内容持续**回流大脑**形成闭环；而行动系统**直接操作企业现有系统**（如更新 ERP）。
>
> 本方案不是纸上概念——文中"本体"层的对象/关系/动作/函数、数据集成、双服务联动(流程/报表)、数据权限等能力，**已在 cmx-ontology 平台真机跑通**（见文末"落地现状"）。

${img('hero', '数字生命体总览')}

---

## 目录
1. [立意与范式：为什么是"生命体"](#一立意与范式)
2. [总体架构：四层解剖](#二总体架构四层解剖)
3. [认知闭环：感知→思考→决策→行动→学习](#三认知闭环)
4. [大脑：大模型层](#四大脑大模型层)
5. [小脑：智能体层](#五小脑智能体层)
6. [肌体：本体层（感官 · 肌肉 · 神经）](#六肌体本体层)
7. [行动系统：操作企业现有系统](#七行动系统操作企业现有系统)
8. [反馈闭环与记忆进化](#八反馈闭环与记忆进化)
9. [安全 · 治理 · 五层护栏](#九安全治理五层护栏)
10. [端到端场景：一句话关账](#十端到端场景)
11. [分阶段建设路线图](#十一分阶段建设路线图)
12. [技术选型与落地现状](#十二技术选型与落地现状)

---

## 一、立意与范式

传统企业信息化是"**系统的堆叠**"：ERP、CRM、OA、报表、流程各自为政，靠集成中间件生硬缝合，人在中间做"胶水"。而大模型时代的机会，是把企业重新组织成一个**能思考、会指挥、能动手、可进化的有机体**。

用户提出的生物学隐喻精确地刻画了三者的分工与协作：

| 生物学器官 | 系统角色 | 职责 | 本方案对应实现 |
|---|---|---|---|
| **大脑** | 大模型 LLM | 思考、推理、规划、理解意图 | GPT/Claude 等 + 本体 RAG + 记忆 |
| **小脑** | 智能体 Agent | 协调、编排、指挥行动、把想法变成动作序列 | Plan-Execute / ReAct 编排器 + HITL |
| **四肢(手/脚/臂/腿)** | 本体·行动系统 | 动手做事，操作企业系统 | 动作引擎 O4 + 副作用 dispatcher |
| **感官(耳/眼/口/舌)** | 本体·感知系统 | 读取世界、感知企业数据 | 数据集成 O3 + 对象集/函数 O5 + 搜索 |
| **神经系统** | 本体·语义/事件 | 传导信号、连接全身、维持一致 | 语义本体(对象/关系) + 事件总线/Outbox |
| **免疫系统** | 数据权限/治理 | 抵御越权、保护机体 | PDP/PEP + 审计 + 五层护栏 |

**关键洞察**：真正稀缺的不是"更强的大脑"（大模型是通用的、可外购的），而是给大脑装上**可靠的身体**——一个**语义清晰、动作受治理、能安全触达企业系统**的本体。**本体，就是企业智能的"身体"**；没有身体，大脑只能空想，无法行动。

---

## 二、总体架构：四层解剖

系统自上而下四层，纵向由一条"感知-行动神经"贯通（指令向下、反馈向上）：

${img('arch', '四层架构')}

- **思考层(大脑/LLM)**：把人的自然语言意图，结合本体提供的世界模型，推理出**结构化的"行动意图"**。
- **决策层(小脑/Agent)**：把意图**编排**成一串具体动作，做工具选择、依赖调度、人在环审批、失败补偿——它不"想"业务对不对，它负责"把事办成、办对流程"。
- **肌体层(本体)**：真正的**执行与感知底座**。对上暴露"能感知什么、能做什么"（工具面），对下把动作落到企业系统、把数据读进来。
- **执行对象层(企业系统)**：ERP/CRM/OA/流程/报表/数据库……它们既是**行动的对象**（被更新），也是**感知的来源**（被读取）。

> 分层的价值：**大脑可替换**（换更强的模型不动身体），**身体可复用**（同一套本体，不同大脑/多智能体共享），**风险可隔离**（危险操作被本体的治理层拦在最后一米）。

---

## 三、认知闭环

生命体的智能，本质是一个**永不停歇的闭环**：感知世界 → 思考理解 → 决策规划 → 行动落地 → 反馈学习。这正是企业智能体的"OODA 循环"。

${img('loop', '认知闭环')}

| 阶段 | 主体 | 做什么 | 载体 |
|---|---|---|---|
| **感知 Sense** | 本体·感官 | 读企业数据：对象集查询、函数计算、搜索钻取 | O3 数据集成 · O5 函数 · Search-Around |
| **认知 Think** | 大脑·LLM | 理解意图 + 结合本体上下文推理 | RAG(本体 schema/实例) + 工具目录 |
| **决策 Decide** | 小脑·Agent | 编排动作序列 + 人在环闸门 | Plan-Execute + dry-run + HITL |
| **行动 Act** | 本体·肌肉 | 写回对象 + 触发副作用操作企业系统 | O4 动作引擎 + Outbox dispatcher |
| **学习 Learn** | 反馈回流 | 结果回喂大脑，沉淀记忆、优化后续 | 事件流/SSE + 记忆库 |

**闭环中枢是本体**：它既是感知的出口，也是行动的入口，还是把信号可靠传导的神经——所以本体做扎实，闭环才转得动。

---

## 四、大脑：大模型层

大脑负责**"想"**，但它的想法必须**可执行、可验证、可治理**。设计要点：

**4.1 意图而非代码**
- 大模型的输出**不是直接的 SQL / 代码 / API 调用**，而是**结构化的行动意图**（调哪个本体动作、传哪些参数、期望什么结果）。
- 意图交给小脑去编排、交给本体去执行——大脑不直接碰企业系统，**风险被隔离**。

**4.2 本体作为大脑的"世界模型"(RAG)**
- 大脑看不懂几百张 ERP 物理表，但看得懂**本体的语义模型**：对象"订单"有哪些属性、和"客户"什么关系、有哪些"动作"可做。
- 把**本体 schema + 相关对象实例 + 可用工具目录**作为上下文喂给大脑（检索增强），大脑的推理就**扎根于企业真实语义**，而非幻觉。

**4.3 记忆**
- **短时记忆**：会话上下文（当前任务、已执行动作、中间结果）。
- **长时记忆**：向量库(语义检索历史) + 图谱(本体本身就是企业知识图谱) + 结构化审计(做过什么、结果如何)。
- 记忆让大脑**越用越懂这家企业**。

---

## 五、小脑：智能体层

小脑负责**"把想法变成有序、安全、办得成的行动"**。它是大脑与本体之间的"运动皮层"。

**5.1 编排范式**：Plan-Execute（先规划整条动作链，再逐步执行）为主，ReAct（走一步看一步）为辅；复杂任务拆子任务、建依赖图、并发调度。

**5.2 工具 = 本体能力**：智能体的"工具箱"**不是一堆手写 API**，而是**本体自动暴露的能力面**——每个"动作/函数/对象集"就是一个强类型工具（含参数形状、前置校验、副作用声明）。本体新增一个动作，智能体就自动多一个工具，**零胶水**。

**5.3 人在环(HITL)与安全提交**：
- 高风险动作（改 ERP、发起流程、大额过账）先 **dry-run 预演**，把"将要改什么"呈现给人**确认后再提交**。
- 失败**重试 / 补偿 / 回滚**；一切**事务性 + 可审计**。

**5.4 多智能体**：财务体、供应链体、风控体……各有专精，经本体这套"共享神经"协作。

---

## 六、肌体：本体层（感官 · 肌肉 · 神经）

**本体是这套系统的身体**，也是本方案的重心。把 Palantir 式本体的每一类元素，映射为生命体的一个系统：

${img('anatomy', '本体解剖')}

- **神经（语义）**：对象类型=器官、关系=神经连接、接口=横切系统、共享属性=基因——它们构成企业的**统一语义层**，让全身信号一致。
- **感官（感知）**：函数(O5)=反射/感官、数据集成(O3)=消化摄入——把外部世界读进来、算出来、回传给大脑。
- **肌肉（行动）**：动作(O4)=肌肉、副作用=肌腱末梢、事件/Outbox=神经递质——把决策**可靠地**变成对企业系统的操作。
- **免疫（治理）**：数据权限 PDP/PEP=免疫系统——抵御越权，保护机体。

> 为什么用"本体"而不是直接让大脑调 API？因为 **API 是无语义的、脆弱的、不受治理的**；而本体是**强类型、语义化、带前置校验和事务/审计的受治理入口**。给大脑一堆裸 API，等于给一个天才装了会乱抓的手；给它一套本体，才是装上**受控的、有本体感的肢体**。

---

## 七、行动系统：操作企业现有系统

这是"能动手"的关键——**一个业务动作，扇出多路副作用，触达企业各系统**，且事务性、可治理、可观测：

${img('action', '行动系统集成')}

**机制**：动作引擎(O4) 在**单事务**内完成"改本体对象 + 把副作用写进 Outbox"；**dispatcher** 再从 Outbox 可靠分派（SKIP LOCKED 并发安全、失败重试、熄火可控、SSRF 白名单护栏、错误如实回传）。

**已真机打通的副作用**：
- \`startBusinessProcess\` → **流程引擎** 真起审批流实例；
- \`computeReport\` → **报表平台** 真算落库(cr_cell_data)；
- \`webhook\` → 三方系统 HTTP（受白名单约束）；
- \`modifyObject\` → 本体对象写回；\`notification\` → 通知/待办。

**规划中的连接器**：ERP 过账/单据更新、主数据同步、CRM/MES 写回——**同一套 dispatcher 模式**，加一类连接器即可，身体"长出新的手"。

> **"更新 ERP"如何发生**：大脑说"关账"，小脑编排"关账联动动作"，本体动作在事务内改对象 + 入 Outbox，dispatcher 调 ERP 连接器过账、调流程起审批、调报表出表——**一次决策，多系统协同落地**，且每一步可审计、可回滚。

---

## 八、反馈闭环与记忆进化

**行动/感知的结果必须回流大脑**，否则只是开环的自动化，不是"智能"：

- **感知回流**：对象集/函数的结果、搜索钻取的发现 → 作为上下文回喂大脑。
- **行动回流**：动作执行结果（成功/失败/业务错误消息）、流程实例状态、报表计算结果 → 经**事件流/SSE** 回到大脑，让它知道"办得怎么样"。
- **沉淀为记忆**：每次闭环的输入/决策/结果落审计 + 向量化，**下次更准**。
- **进化**：从"人下指令→执行"，逐步到"智能体**主动感知异常→主动提议行动→人确认**"，最终是**自愈 + 主动洞察**的有机体。

---

## 九、安全 · 治理 · 五层护栏

给大脑装身体，最大的风险是"乱动手"。必须五层护栏层层设防：

| # | 护栏 | 位置 | 作用 |
|---|---|---|---|
| L1 | **意图约束** | 大脑输出 | 只能产出"本体已声明的动作/参数"，不能凭空造 API |
| L2 | **编排闸门(HITL)** | 小脑 | 高风险动作 dry-run 预演 → 人确认 → 才提交 |
| L3 | **动作治理** | 本体 O4 | 参数校验 + FEEL 前置校验 + 单事务 + 全审计 |
| L4 | **数据权限(PDP/PEP)** | 本体 O6/data-auth | 决策/执行分离 + 残差约束，越权即拒 |
| L5 | **出站护栏** | dispatcher | 服务身份鉴权 + SSRF 白名单 + 熄火开关 + 错误可观测 |

> 落地佐证：本次连通性测试发现**报表服务缺鉴权门**，已**当场补齐**（jwt + 服务 API Key，no-key→401）——这类"免疫缺陷"必须在接入大脑**之前**堵死。

---

## 十、端到端场景

一句自然语言"帮我把 3 月关账并出资产负债表"，如何穿过全身：

${img('scenario', '端到端场景时序')}

每一步都落在真实能力上：读本体(感知)→大脑理解→小脑编排+HITL→本体"关账联动模板"一个动作扇出**起流程 + 算报表**→企业系统真实产生实例与报表数据→结果回流→大脑向用户复述。**这条链路的"本体→企业系统"段已真机跑通**（关账联动模板 E2E：一个动作同时建流程实例 + 落报表数据）。

---

## 十一、分阶段建设路线图

不必一步登天。**先把身体立住（已完成大半），再接大脑/小脑，最后进化为自主有机体**：

${img('roadmap', '建设路线图')}

- **P0 肌体（已完成大半）**：本体平台——对象/关系/动作/函数、数据集成、数据权限、**双服务联动(流程/报表)真机跑通**。
- **P1 神经（进行中）**：ERP/CRM 连接器、事件总线与反馈回流、把本体只读能力封装为 **MCP/工具**供大脑调用。
- **P2 小脑**：Agent 编排器——工具=本体动作/函数、ReAct+HITL、dry-run→提交+补偿。
- **P3 大脑**：LLM 接入——NL→意图→动作、本体 RAG 上下文、记忆(向量+图谱)。
- **P4 有机体**：结果回流学习、多智能体协作、自愈与主动洞察。

> **风险最低的顺序**：身体先于大脑。一个"能被安全操作、语义清晰"的本体，即使暂时没接大模型，本身就是巨大的中台价值；接上大脑后，价值再上一个数量级。

---

## 十二、技术选型与落地现状

**分层选型建议**

| 层 | 选型方向 | 说明 |
|---|---|---|
| 大脑 | Claude / GPT 等 + RAG | 通用大模型，经本体 schema/实例检索增强 |
| 小脑 | Plan-Execute/ReAct 编排 + MCP | 工具即本体能力；HITL 审批；补偿事务 |
| 本体 | Palantir 式本体平台(Rust) | 对象/关系/动作/函数 + Funnel + PDP/PEP + Outbox |
| 神经 | 事件总线 + Outbox + SSE | 事务性投递、反馈回流 |
| 连接器 | dispatcher + 每系统一 connector | 起流程/算报表/webhook/ERP 过账 |
| 治理 | 五层护栏 + 审计 + data-auth | 意图约束→HITL→动作治理→数据权限→出站护栏 |

**落地现状（真机已验证的"肌体"能力）**
- ✅ 本体平台 O0–O8 主线 + P0/P1/P2：对象/关系/接口/共享属性/**动作(O4)**/**函数(O5)**、数据集成(O3)、对象浏览器、OSDK、客户 360 工作台。
- ✅ **行动系统真机打通**：动作副作用 → **流程引擎**(真起实例) + **报表平台**(真算落库) + webhook + 通知，事务性 Outbox + dispatcher。
- ✅ **可视化配置**：设计台可视化配"起流程/生成报表/Webhook/通知"副作用 + **关账联动模板**（一个动作串流程+报表，从模板一键新建）。
- ✅ **连通性 + 治理**：五微服务(门户/本体/流程/报表/权限)贯通；连通性 53 断言全通过；发现并**补齐报表鉴权门**。
- 🔜 **待接**：大脑(LLM)、小脑(Agent 编排)、ERP 连接器、反馈回流学习——即本方案 P1–P4。

---

### 结语

> 大模型给了企业一颗**通用的大脑**；但企业智能的胜负手，在于能否为这颗大脑装上一副**语义清晰、动作受治理、能安全触达一切现有系统**的**身体**——这就是**本体**。
>
> 我们已经把这副身体的**肌肉、感官、神经**造了出来并真机跑通；接下来，是让大脑与小脑入驻，让这个**数字生命体**真正地感知、思考、行动、进化。

<div align="right"><sub>本文所有插图为内嵌 base64 SVG（<code>docs/assets/organism/build.cjs</code> 生成）· 图文并茂 · 可离线阅读</sub></div>
`;

const out = path.resolve(__dirname, '../../20260901_大模型+智能体+本体_企业智能系统建设方案.md');
fs.writeFileSync(out, md);
// 顺带各 SVG 单独落盘，便于复用/微调
const svgDir = path.resolve(__dirname, 'svg'); fs.mkdirSync(svgDir, { recursive: true });
for (const k in D) fs.writeFileSync(path.join(svgDir, k + '.svg'), D[k]);
console.log('written:', out);
console.log('bytes:', fs.statSync(out).size, '| diagrams:', Object.keys(D).join(', '));
