/* 生成《企业智能落地·务实版(对抗性检验后)》——图文并茂 markdown，内嵌 base64 SVG。
 * 与 build.cjs(生命体版)并存，不覆盖。运行：node docs/assets/organism/build2.cjs
 */
'use strict';
const fs = require('fs'), path = require('path');
const FONT = "'Inter','Segoe UI','PingFang SC','Microsoft YaHei',system-ui,sans-serif";
const C = {
  bg0: '#070b16', bg1: '#0b1120', card: '#0e1526', card2: '#121b30', stroke: '#1c2740', stroke2: '#2b3a5c',
  fg: '#e8eefc', mut: '#9fb0cc', dim: '#5f7290',
  ok: '#34d399', okD: '#047857', warn: '#fbbf24', warnD: '#b45309', bad: '#fb7185', badD: '#be123c',
  blue: '#38bdf8', blueD: '#0369a1', violet: '#a78bfa', slate: '#64748b',
};
const esc = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
function T(x, y, s, o = {}) { const { size = 15, fill = C.fg, anchor = 'start', w = 400, ls = 0, op = 1 } = o; return `<text x="${x}" y="${y}" font-size="${size}" fill="${fill}" text-anchor="${anchor}" font-weight="${w}" letter-spacing="${ls}" opacity="${op}">${esc(s)}</text>`; }
function R(x, y, w, h, o = {}) { const { r = 12, fill = 'none', stroke = 'none', sw = 1, op = 1, dash = '' } = o; return `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${r}" fill="${fill}" stroke="${stroke}" stroke-width="${sw}" opacity="${op}"${dash ? ` stroke-dasharray="${dash}"` : ''}/>`; }
function CI(cx, cy, r, o = {}) { const { fill = 'none', stroke = 'none', sw = 1, op = 1 } = o; return `<circle cx="${cx}" cy="${cy}" r="${r}" fill="${fill}" stroke="${stroke}" stroke-width="${sw}" opacity="${op}"/>`; }
function L(x1, y1, x2, y2, o = {}) { const { stroke = C.stroke2, sw = 1.4, dash = '', op = 1, cap = 'round', mk = '' } = o; return `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${stroke}" stroke-width="${sw}" opacity="${op}" stroke-linecap="${cap}"${dash ? ` stroke-dasharray="${dash}"` : ''}${mk ? ` marker-end="url(#${mk})"` : ''}/>`; }
function P(d, o = {}) { const { stroke = 'none', sw = 1.6, fill = 'none', dash = '', op = 1, cap = 'round', mk = '' } = o; return `<path d="${d}" stroke="${stroke}" stroke-width="${sw}" fill="${fill}" opacity="${op}" stroke-linecap="${cap}" stroke-linejoin="round"${dash ? ` stroke-dasharray="${dash}"` : ''}${mk ? ` marker-end="url(#${mk})"` : ''}/>`; }
function chip(x, y, w, h, label, col, o = {}) { const { sub = '', r = 11, fill = C.card2, tsize = 13.5, icon = '●' } = o; let s = R(x, y, w, h, { r, fill, stroke: col, sw: 1.3 }); s += T(x + 15, y + (sub ? h / 2 - 1 : h / 2 + 5), (icon === '✓' ? '✓ ' : '') + label, { size: tsize, fill: C.fg, w: 600 }); if (sub) s += T(x + 15, y + h / 2 + 14, sub, { size: 11, fill: C.mut }); s += CI(x + w - 16, y + h / 2, 4, { fill: col }); return s; }
function badge(x, y, txt, col) { const w = 18 + txt.length * 8.6; return R(x, y, w, 23, { r: 11, fill: col + '22', stroke: col, sw: 1 }) + T(x + w / 2, y + 15.5, txt, { size: 12, anchor: 'middle', fill: col, w: 700 }); }
function grad(id, c1, c2, v = true) { return `<linearGradient id="${id}" x1="0" y1="0" x2="${v ? 0 : 1}" y2="${v ? 1 : 0}"><stop offset="0" stop-color="${c1}"/><stop offset="1" stop-color="${c2}"/></linearGradient>`; }
function markers() { const mk = (id, col) => `<marker id="${id}" markerWidth="9" markerHeight="9" refX="7" refY="4" orient="auto"><path d="M0.5,0.5 L8,4 L0.5,7.5 L3,4 Z" fill="${col}"/></marker>`; return mk('aFg', C.mut) + mk('aOk', C.ok) + mk('aWarn', C.warn) + mk('aBad', C.bad) + mk('aBlue', C.blue); }
function DEFS() { return `<defs><linearGradient id="bgG" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${C.bg1}"/><stop offset="1" stop-color="${C.bg0}"/></linearGradient>${grad('okG', '#6ee7b7', C.okD)}${grad('warnG', '#fcd34d', C.warnD)}${grad('badG', '#fda4af', C.badD)}${grad('blueG', '#7dd3fc', C.blueD)}${markers()}</defs>`; }
function svg(W, H, inner) { return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" font-family="${FONT}">${DEFS()}<rect x="1.5" y="1.5" width="${W - 3}" height="${H - 3}" rx="24" fill="url(#bgG)" stroke="#18223c"/>${inner}</svg>`; }
function title(x, y, t, sub) { return T(x, y, t, { size: 26, w: 800, fill: C.fg }) + (sub ? T(x, y + 25, sub, { size: 14, fill: C.mut }) : ''); }

// D1 现状诚实地图
function realityMap() {
  const W = 1200, H = 700; let s = title(44, 58, '现状诚实地图 · 别让信心从左列溢价到右列', '把每项能力如实贴标：已验证 / 未验证的假设 / 未建或高风险——区分"做过"和"以为能做"');
  const cols = [
    ['已验证 · 真机跑通', C.ok, 'okG', [
      ['本体：对象/关系/动作O4/函数O5', '强类型 + 前置校验 + 事务 + 审计'],
      ['数据集成 O3 · 数据权限 PDP/PEP', 'report 鉴权门本次刚补齐'],
      ['双服务联动(flow/report)', 'startBusinessProcess + computeReport 真机'],
      ['出站护栏 + 事务性 Outbox + dispatcher', 'SSRF白名单 · 失败/熄火 · 错误回传'],
      ['可视化配置 + 关账联动模板', '一动作串流程+报表(注意爆炸半径)'],
    ]],
    ['假设 · 未经验证', C.warn, 'warnG', [
      ['LLM 意图→动作的可靠性', '多步复利误差未量化(见图3)'],
      ['本体 RAG 上下文"够用"', '大 schema 的检索/裁剪未验证'],
      ['"反馈=学习"', '权重冻结，实为塞 RAG，非进化'],
      ['HITL 不退化成橡皮图章', '规模化后疲劳性盖章风险'],
    ]],
    ['未建 · 或高风险', C.bad, 'badG', [
      ['Agent 编排器(小脑)', '整章未写，最难的部分'],
      ['写系统级记录(ERP 过账)', 'SOX/SoD/审计雷区，非技术问题'],
      ['多步自主可靠性', '不可逆动作 × 复利误差'],
      ['自愈 / 主动洞察(P4)', '相对已展示能力=科幻'],
    ]],
  ];
  const cw = 372, gap = 12, x0 = 44, y0 = 96;
  cols.forEach((col, ci) => {
    const x = x0 + ci * (cw + gap);
    s += R(x, y0, cw, 40, { r: 12, fill: `url(#${col[2]})` });
    s += T(x + 18, y0 + 26, col[0], { size: 15.5, w: 800, fill: '#0b1120' });
    let y = y0 + 52;
    col[3].forEach(it => { const h = 52; s += R(x, y, cw, h, { r: 11, fill: C.card, stroke: col[1], sw: 1.1 }); s += CI(x + 16, y + 20, 4, { fill: col[1] }); s += T(x + 30, y + 24, it[0], { size: 13, w: 600, fill: C.fg }); s += T(x + 30, y + 41, it[1], { size: 11, fill: C.mut }); y += h + 8; });
  });
  s += T(44, 686, '⚠ 规则：左列的"做过"绝不能给中/右列的"以为能做"背书。整套系统的价值，卡在最右列那些最难、最没做的部分。', { size: 12.5, fill: C.warn, w: 600 });
  return svg(W, H, s);
}

// D2 价值切片（月末关账，写侧设闸）
function valueSlice() {
  const W = 1200, H = 640; let s = title(44, 58, '楔子：只切一条纵向价值切片(月末关账)', '不铺平台。一条工作流端到端跑通、量化 ROI，再谈横向——写系统级记录处显式设人闸');
  const lanes = [['感知(读)', C.blue], ['LLM 建议', C.violet], ['人工审批', C.warn], ['执行·已验证', C.ok], ['ERP 过账·高风险', C.bad]];
  const bw = 210, gap = 18, x0 = 44, y = 110;
  lanes.forEach((l, i) => {
    const x = x0 + i * (bw + gap);
    const fill = i === 3 ? 'url(#okG)' : i === 4 ? C.card2 : C.card2;
    s += R(x, y, bw, 84, { r: 14, fill, stroke: l[1], sw: 1.5, dash: i === 4 ? '6 5' : '' });
    s += T(x + bw / 2, y + 34, l[0], { size: 15, anchor: 'middle', w: 800, fill: i === 3 ? '#0b1120' : l[1] });
    s += T(x + bw / 2, y + 58, ['对象集/函数只读', 'org+period→意图', 'dry-run→确认', 'flow起实例+report出表', '过账 = 不可逆'][i], { size: 11, anchor: 'middle', fill: i === 3 ? '#0b3b2a' : C.mut });
    if (i < 4) s += L(x + bw, y + 42, x + bw + gap, y + 42, { stroke: i === 3 ? C.bad : C.mut, sw: 2, mk: i === 3 ? 'aBad' : 'aFg' });
  });
  // 已验证段标注
  s += R(x0, y + 100, (bw + gap) * 4 - gap, 30, { r: 9, fill: C.ok + '14', stroke: C.ok, sw: 1, dash: '4 4' });
  s += T(x0 + 14, y + 120, '① 这一段"本体→flow/report"已真机跑通(关账联动模板)——安全区，可先上', { size: 12.5, fill: C.ok, w: 600 });
  // ERP 闸门
  const gx = x0 + 4 * (bw + gap);
  s += R(gx, y + 100, bw, 116, { r: 12, fill: C.bad + '10', stroke: C.bad, sw: 1.3 });
  s += T(gx + 12, y + 122, '② ERP 过账闸门', { size: 13.5, w: 800, fill: C.bad });
  ['先降级为"生成分录草稿"', '人在 ERP 内执行(职责分离)', '远期才谈受控自动写', '须双人复核 + 完整审计'].forEach((t, i) => s += T(gx + 12, y + 144 + i * 18, '· ' + t, { size: 11.5, fill: C.mut }));
  // 对比：平台优先 vs 切片优先
  s += R(44, 500, 1112, 92, { r: 14, fill: C.card, stroke: C.stroke });
  s += T(64, 528, '为什么不"先铺四层平台"?', { size: 14, w: 800, fill: C.fg });
  s += T(64, 552, '没有真实 agent 用例，你根本不知道该建什么本体/动作——先铺平台=大概率建错身体、过度建模、又漏建真正需要的。', { size: 12.8, fill: C.mut });
  s += T(64, 574, '切片优先：一条纵切验证价值与失败处理 → ROI 达标再横向复制；每片都能独立止损。', { size: 12.8, fill: C.ok, w: 600 });
  return svg(W, H, s);
}

// D3 可靠性数学
function reliabilityMath() {
  const W = 1200, H = 640; let s = title(44, 58, '可靠性数学：为什么"长链自主"必然崩', '端到端成功率 = 单步可靠率 ^ 步数。这条曲线，上一版只字未提');
  const ox = 110, oy = 520, pw = 720, ph = 400;
  // 轴
  s += L(ox, oy, ox + pw, oy, { stroke: C.stroke2, sw: 1.4 });
  s += L(ox, oy, ox, oy - ph, { stroke: C.stroke2, sw: 1.4 });
  [0, 25, 50, 75, 100].forEach(p => { const yy = oy - p / 100 * ph; s += L(ox - 5, yy, ox + pw, yy, { stroke: C.stroke, sw: .8, dash: '2 6', op: .5 }); s += T(ox - 12, yy + 4, p + '%', { size: 11.5, anchor: 'end', fill: C.mut }); });
  const N = 12; const xF = n => ox + (n - 1) / (N - 1) * pw, yF = p => oy - p * ph;
  for (let n = 1; n <= N; n++) { if (n === 1 || n % 2 === 1 || n === N) s += T(xF(n), oy + 20, '' + n, { size: 11.5, anchor: 'middle', fill: C.mut }); }
  s += T(ox + pw / 2, oy + 42, '动作链步数 n →', { size: 12.5, anchor: 'middle', fill: C.mut });
  const curves = [[0.99, C.ok, 'r=0.99 每步'], [0.95, C.warn, 'r=0.95'], [0.90, C.bad, 'r=0.90']];
  curves.forEach(cv => {
    let d = ''; for (let n = 1; n <= N; n++) { const p = Math.pow(cv[0], n); d += (n === 1 ? 'M' : 'L') + xF(n) + ',' + yF(p) + ' '; }
    s += P(d, { stroke: cv[1], sw: 2.6 });
    const pEnd = Math.pow(cv[0], N); s += CI(xF(N), yF(pEnd), 4, { fill: cv[1] });
    s += T(xF(N) + 8, yF(pEnd) + 4, Math.round(pEnd * 100) + '%', { size: 12, fill: cv[1], w: 700 });
  });
  // 标注关键点
  s += CI(xF(10), yF(Math.pow(0.9, 10)), 5, { fill: 'none', stroke: C.bad, sw: 2 });
  s += T(xF(10) - 6, yF(Math.pow(0.9, 10)) - 14, '0.90^10≈35%', { size: 11.5, anchor: 'end', fill: C.bad });
  // 图例 + 结论
  curves.forEach((cv, i) => { s += CI(ox + 12, oy - ph + 18 + i * 22, 5, { fill: cv[1] }); s += T(ox + 26, oy - ph + 22 + i * 22, cv[2], { size: 12, fill: C.mut }); });
  const bx = 880;
  s += R(bx, 110, 276, 420, { r: 14, fill: C.card, stroke: C.blue, sw: 1.3 });
  s += T(bx + 18, 140, '结论', { size: 15, w: 800, fill: C.blue });
  ['单步再可靠，链一长就崩：', '· 0.95 单步 → 10 步仅 ~60%', '· 财务/不可逆动作不容 40% 翻车', '', '对策：不是"追求更长自主"，', '而是缩短自主步长 + 在', '不可逆步骤前设「人闸门」，', '让误差累积在闸门处重置。', '', 'HITL 不是可选项，是数学', '逼出来的必需品——但它也', '意味着：高风险处 AI 自主性', '≈ 0。这是内在张力，非 bug。'].forEach((t, i) => s += T(bx + 18, 168 + i * 27, t, { size: 12.3, fill: i > 3 && i < 8 ? C.warn : C.mut, w: t.startsWith('对策') || t.startsWith('HITL') ? 700 : 400 }));
  return svg(W, H, s);
}

// D4 写回阶梯
function writeLadder() {
  const W = 1200, H = 560; let s = title(44, 58, '写系统级记录的阶梯 · 从只读到过账，逐级加闸', '"更新 ERP"不是一句话的卖点，是一条从安全到危险的阶梯——当前只应站在下三级');
  const rungs = [
    ['① 只读感知', '读 ERP / 生成对象集 / 函数计算', C.ok, '安全'],
    ['② 生成建议(草稿)', 'AI 备好分录/单据草稿，不写任何系统', C.ok, '安全'],
    ['③ 人工执行', '人在 ERP 内亲手执行，AI 只做辅助', C.ok, '安全·当前上限'],
    ['④ 受控自动写·可逆', '自动写可撤销的更新 + 立即可回滚', C.warn, '需设计'],
    ['⑤ 受控自动写·不可逆(GL过账)', '须 SoD 职责分离 + 双人复核 + 完整审计 + 监管合规', C.bad, '远期·勿轻言'],
  ];
  const n = rungs.length, bw = 640, bh = 56, x0 = 120, y0 = 106, dx = 78, dy = 74;
  rungs.forEach((r, i) => {
    const x = x0 + i * dx, y = y0 + i * dy;
    s += R(x, y, bw, bh, { r: 12, fill: C.card, stroke: r[2], sw: 1.4 });
    s += CI(x + 20, y + bh / 2, 5, { fill: r[2] });
    s += T(x + 36, y + 24, r[0], { size: 14, w: 700, fill: C.fg });
    s += T(x + 36, y + 43, r[1], { size: 11.5, fill: C.mut });
    s += badge(x + bw - 96, y + bh / 2 - 11, r[3], r[2]);
    if (i < n - 1) s += L(x + 30, y + bh, x + dx + 30, y + dy, { stroke: C.stroke2, sw: 2, dash: '3 5' });
  });
  s += R(760, 380, 396, 130, { r: 12, fill: C.bad + '10', stroke: C.bad, sw: 1.2 });
  s += T(778, 408, '危险区 (④⑤)', { size: 14, w: 800, fill: C.bad });
  ['"一个 AI 决定这么过账"', '不是可接受的审计轨迹。', '拦路虎是组织/合规/法律，', '不是技术——上一版没提。'].forEach((t, i) => s += T(778, 434 + i * 20, t, { size: 12.2, fill: C.mut }));
  return svg(W, H, s);
}

// D5 saga 失败/补偿矩阵
function sagaMatrix() {
  const W = 1200, H = 600; let s = title(44, 58, '这是 saga，不是事务 · 失败与补偿必须显式设计', '跨服务扇出非原子。"dispatched=N"的幸福路径，掩盖了部分失败→跨系统不一致');
  // 管道
  const steps = ['①改本体对象', '②起流程', '③算报表', '④ERP过账'];
  const sx = 60, sw = 250, sy = 108, gap = 12;
  steps.forEach((st, i) => { const x = sx + i * (sw + gap); const col = i === 0 ? C.ok : C.warn; s += R(x, sy, sw, 52, { r: 11, fill: C.card2, stroke: col, sw: 1.3 }); s += T(x + sw / 2, sy + 32, st, { size: 14, anchor: 'middle', w: 700, fill: C.fg }); if (i < 3) s += L(x + sw, sy + 26, x + sw + gap, sy + 26, { stroke: C.mut, sw: 2, mk: 'aFg' }); });
  // 本地事务框
  s += R(sx - 6, sy - 8, sw + 12, 68, { r: 13, fill: 'none', stroke: C.ok, sw: 1.4, dash: '5 4' });
  s += T(sx, sy - 14, '本地事务(原子)', { size: 11.5, fill: C.ok, w: 600 });
  s += R(sx + sw + gap - 6, sy - 8, (sw + gap) * 3, 68, { r: 13, fill: 'none', stroke: C.warn, sw: 1.4, dash: '5 4' });
  s += T(sx + sw + gap + 4, sy - 14, '最终一致 · 非原子 · 可部分失败 ⚠', { size: 11.5, fill: C.warn, w: 600 });
  // 矩阵
  const rows = [['④过账失败', '①②③已生效，④未', '撤流程/对冲 · 标记待过账 · 告警 · 转人工', C.bad], ['③报表失败', '①②生效，③④未', '重算(幂等) · 阻断④ · 通知', C.warn], ['②起流程失败', '①生效，②③④未', '回滚④触发 · 重试/死信队列 · 对账', C.warn], ['dispatcher 宕', '已入 Outbox 未投递', 'SKIP LOCKED 续投 · reaper 兜底(已实现)', C.ok]];
  const my = 220, rh = 76;
  s += T(60, my - 12, '部分失败 → 状态 → 补偿：', { size: 13.5, w: 700, fill: C.fg });
  rows.forEach((r, i) => { const y = my + i * rh; s += R(60, y, 1080, rh - 10, { r: 11, fill: i % 2 ? C.card : C.card2, stroke: C.stroke }); s += CI(84, y + 33, 6, { fill: r[3] }); s += T(104, y + 30, r[0], { size: 13.5, w: 700, fill: r[3] }); s += T(104, y + 50, '故障点', { size: 10.5, fill: C.dim }); s += T(300, y + 39, r[1], { size: 12.8, fill: C.mut }); s += T(560, y + 39, '→  ' + r[2], { size: 12.8, fill: C.fg }); });
  return svg(W, H, s);
}

// D6 本体的真实价值
function ontologyValue() {
  const W = 1200, H = 520; let s = title(44, 58, '本体的可辩护价值：受治理的动作边界，不是语义乌托邦', '别为"统一语义层"辩护(那是历史坟场)；为"把不可信大脑的动作拦在最后一米"辩护');
  const cols = [
    ['裸 API 直调(LLM → API)', C.bad, ['无 schema：LLM 靠猜参数', '无前置校验：错参直达生产', '无事务：半成品状态', '无审计：谁决定的?说不清', '无权限门：越权无感', '幻觉 → 直接落到系统'], '风险：忠实执行一个会confabulate的大脑'],
    ['受治理动作边界(经本体)', C.ok, ['强类型参数 + 形状约束', 'FEEL 前置校验闸', '本地事务 + 幂等', '全程审计(动作/参数/结果)', 'PDP/PEP 数据权限', '幻觉 → 被拦在最后一米'], '价值：动作可治理、可审计、可回滚'],
  ];
  const cw = 540, x0 = 44, y0 = 104;
  cols.forEach((col, ci) => {
    const x = x0 + ci * (cw + 32);
    s += R(x, y0, cw, 360, { r: 16, fill: C.card, stroke: col[1], sw: 1.5 });
    s += R(x, y0, cw, 48, { r: 16, fill: col[1] + '18' });
    s += T(x + 20, y0 + 31, col[0], { size: 15.5, w: 800, fill: col[1] });
    col[2].forEach((t, i) => { s += (ci ? '✓' : '✕'); s += T(x + 24, y0 + 84 + i * 38, (ci ? '✓ ' : '✕ ') + t, { size: 13.2, fill: ci ? C.fg : C.mut, w: 500 }); });
    s += R(x + 16, y0 + 306, cw - 32, 40, { r: 10, fill: col[1] + '12' });
    s += T(x + 30, y0 + 331, col[3], { size: 12.5, fill: col[1], w: 600 });
  });
  s += T(44, 500, '注：这个价值与"生命体隐喻"无关，拆掉隐喻它照样成立——它是纯工程与治理的资产。', { size: 12.5, fill: C.mut });
  return svg(W, H, s);
}

// D7 务实路线图（切片 + 止损闸）
function pragmaticRoadmap() {
  const W = 1200, H = 520; let s = title(44, 58, '务实路线图 · 薄纵切片，每片可独立止损', '不是 P0→P4 横向铺层；是一片一片纵切，每片之间有"决策/止损闸"');
  const slices = [
    ['切片 1', '月末关账(读+建议+人执行)', C.ok, ['最小本体(已具备)', '最小 agent 编排', '写侧只到"③人执行"'], '先上'],
    ['切片 2', '同工作流 +可逆自动写', C.warn, ['④受控自动写', '补偿/对账矩阵', 'SoD 设计'], '过闸再上'],
    ['切片 3', '第二个高价值工作流', C.blue, ['复用护栏/审计', '横向复制模式', '扩连接器'], '复制'],
    ['成熟', '多工作流 + 多智能体', C.violet, ['沉淀记忆', '主动建议(仍人确认)', '谈"自主"须极谨慎'], '愿景'],
  ];
  const bw = 250, gap = 34, x0 = 44, y = 108;
  slices.forEach((p, i) => {
    const x = x0 + i * (bw + gap);
    s += R(x, y, bw, 300, { r: 16, fill: C.card, stroke: p[2], sw: 1.5 });
    s += R(x, y, bw, 60, { r: 16, fill: p[2] + '18' });
    s += T(x + 18, y + 27, p[0], { size: 15, w: 800, fill: p[2] });
    s += T(x + 18, y + 48, p[1], { size: 12, fill: C.mut });
    p[3].forEach((d, j) => { s += CI(x + 21, y + 92 + j * 34 - 4, 3.5, { fill: p[2] }); s += T(x + 34, y + 96 + j * 34, d, { size: 12, fill: C.mut }); });
    s += badge(x + 18, y + 262, p[4], p[4] === '先上' ? C.ok : p[4] === '愿景' ? C.dim : C.warn);
    if (i < slices.length - 1) { const mx = x + bw + gap / 2; s += P(`M${x + bw},${y + 150} L${x + bw + gap},${y + 150}`, { stroke: C.stroke2, sw: 2, mk: 'aFg' }); s += T(mx, y + 138, '闸', { size: 11, anchor: 'middle', fill: C.warn, w: 700 }); s += CI(mx, y + 150, 12, { fill: 'none', stroke: C.warn, sw: 1.2, dash: '3 3' }); }
  });
  s += T(44, 470, '每个"闸"= 止损点：ROI/可靠性/合规未达标就停在这，不往危险侧走。这才是对抗性思维下的推进方式。', { size: 12.5, fill: C.warn, w: 600 });
  return svg(W, H, s);
}

const D = { reality: realityMap(), slice: valueSlice(), reliability: reliabilityMath(), ladder: writeLadder(), saga: sagaMatrix(), value: ontologyValue(), roadmap: pragmaticRoadmap() };
function img(k, alt) { const b = Buffer.from(D[k]).toString('base64'); return `<div align="center"><img alt="${esc(alt)}" width="1040" src="data:image/svg+xml;base64,${b}"/></div>`; }

const md = `# 企业智能落地 · 务实版（对抗性检验后）

> **这份文档是《大模型+智能体+本体·数字生命体版》的对抗性重写**。生命体版负责"讲愿景、鼓舞人"；这一版负责"泼冷水、不翻车"。两份**并存**——立项讲前者，评审用后者。
>
> **三条硬约束**（回应对上一版的自我批判）：
> 1. **隐喻降级**——"大脑/小脑/身体"只当沟通糖衣，**绝不用来立论**（且它生物学上是错的：小脑不做决策）。
> 2. **切片优先，不铺平台**——只推进一条纵向价值切片，量化 ROI 后再横向。
> 3. **可靠性 / 失败 / 合规写成一等章节**——不靠 demo 幸福路径遮丑。

---

## 0. 先承认三件事

1. **上一版是"先射箭再画靶"**：我们先建了本体平台，再论证"本体是关键"——**结论对建造方过于方便**。本版把这个偏见摊在桌上。
2. **真机跑通的全是最容易的水管**（dispatcher/Outbox/双服务联动）；**最难的（Agent 可靠编排、写系统级记录、反馈学习）一行没写**。信心不能从前者溢价到后者。
3. **趋势可能对"重本体"不利**：模型 + MCP + code/computer-use 在**去中介化**。所以本体的价值**必须**重新锚定在它真正防得住的东西上——**受治理的动作边界**，而不是"统一语义层"（那是几十年的坟场）。

---

## 1. 现状诚实地图

先分清"**做过**"和"**以为能做**"：

${img('reality', '现状诚实地图')}

**读法**：绿色是真金白银已验证；黄色是**没验证的假设**（尤其"LLM 多步可靠""反馈=学习"）；红色是**没做或高风险**（Agent 编排、ERP 写回）。**系统的成败卡在最右列**——而那正是最没做的部分。

---

## 2. 只切一条纵向价值切片

不铺四层平台。选**月末关账**这一条工作流，端到端跑通、**量化 ROI**，再谈横向复制：

${img('slice', '价值切片')}

- **安全段（可先上）**：感知(只读) → LLM 生成关账建议 → 人 dry-run 确认 → 执行"关账联动"（起流程 + 出报表）——**这段已真机跑通**。
- **危险段（设闸）**：**ERP 过账先不做自动写**，降级为"AI 生成分录草稿 + 人在 ERP 内执行"。
- **为什么不先铺平台**：没有真实 agent 用例，你**不知道该建什么本体/动作**——先铺平台=大概率**建错身体**。切片优先，每片能独立止损。

> ROI 怎么量化（示例口径）：关账周期从 N 天→N/2 天、编制/核对人力 ↓X%、错误率 ↓Y%、可审计性 ↑。**跑不出这些数，就别横向复制。**

---

## 3. 可靠性数学：长链自主必然崩

这是上一版**只字未提**、却最致命的一页：

${img('reliability', '可靠性数学')}

**端到端成功率 = 单步可靠率 ^ 步数**。单步 95%，10 步只剩 ~60%；对**不可逆的财务动作**，40% 翻车率是灾难。

**对策不是"追求更长自主"，而是**：
- **缩短自主步长**，把长任务切成短的、可验证的段；
- 在**每个不可逆步骤前设「人闸门」**，让误差累积在闸门处**重置**；
- 承认**内在张力**：越危险的动作越需要人闸 → 高风险处 **AI 自主性 ≈ 0**。这不是 bug，是数学。所谓"全自动关账"在合规上**短期无解**。

---

## 4. 写系统级记录：一条从只读到过账的阶梯

"更新 ERP"被上一版一句话轻飘飘带过——它其实是一条**从安全到危险的阶梯**，当前只应站在下三级：

${img('ladder', '写回阶梯')}

- ①②③（**安全，当前上限**）：只读 / 生成草稿 / 人工执行。
- ④⑤（**危险区**）：受控自动写——**拦路虎是组织/合规/法律，不是技术**。SOX、职责分离(SoD)、"谁决定的"审计轨迹……**"一个 AI 决定这么过账"不是可接受的审计证据**。
- **结论**：把 ERP 自动写**推到最后**、且必须**专门做合规设计**，而不是当卖点。

---

## 5. 这是 saga，不是事务

上一版反复说"单事务"。**真相**：只有"改本体对象 + 写 Outbox"是**本地原子事务**；之后跨服务扇出（起流程/算报表/过账）是**最终一致、非原子、可部分失败**：

${img('saga', 'saga 失败与补偿矩阵')}

- **必须显式设计**：部分失败 → 不一致状态 → 补偿/对账/告警/转人工。
- \`dispatched=2\` 的幸福路径**证明不了**这些失败被处理了。
- 好消息：dispatcher 层的"入 Outbox 未投递"已有 SKIP LOCKED 续投 + reaper 兜底；**坏消息**：跨到 ERP 的对冲/回滚**尚未设计**，这是切片 2 的核心工作，不是切片 1 的附赠。

---

## 6. 本体到底买了什么

**别为"统一语义层"辩护**（历史坟场：语义网/MDM/规范模型大多失败）；**为"把不可信大脑的动作拦在最后一米"辩护**：

${img('value', '本体的真实价值')}

裸 API 直调 = 让一个会 confabulate 的大脑直接碰生产；受治理动作边界 = **强类型 + 前置校验 + 事务 + 审计 + 权限**把幻觉拦下。**这个价值与隐喻无关，拆掉"身体"的说法它照样成立**——它是纯工程与治理资产。

> 但也要诚实：**如果模型足够强 + 连接器足够好 + 一个薄权限门就够**，重本体可能被绕过。**判据**：当"前置校验/事务/审计/权限"这四样用薄中间件也能拿到时，重本体的溢价就消失了。持续盯住这条线。

---

## 7. 务实路线图：薄切片 + 止损闸

不是 P0→P4 横向铺层，是**一片一片纵切，每片之间有决策/止损闸**：

${img('roadmap', '务实路线图')}

**每个闸 = 止损点**：ROI / 可靠性 / 合规**任一未达标就停在这**，不往危险侧走。这比"分四阶段建平台"**难看、但抗揍**。

---

## 8. 决策清单（什么时候该停 / 该转向）

| 信号 | 含义 | 动作 |
|---|---|---|
| 切片 1 跑不出可量化 ROI | 价值假设不成立 | **停**，别横向复制 |
| 薄中间件即可拿到"校验+事务+审计+权限" | 重本体溢价消失 | 转向薄本体 + 强 agent |
| 多步链可靠率压不过阈值 | 长自主不可行 | 缩短步长 + 加人闸，降低自主预期 |
| ERP 写回卡在合规 | 组织/法律不放行 | 永远停在"③人执行"，别硬推自动写 |
| HITL 退化成橡皮图章 | 安全网失效 | 减少动作面、提高每次确认的信息密度 |

---

## 结语（务实版）

> 大模型给了通用大脑；但企业里，**胜负手不在"更聪明的大脑"，而在"能不能把一个不可信大脑的行动，安全地限制在可治理、可审计、可回滚的边界内"**。
>
> 我们已经把这个"边界"（动作引擎 + 护栏 + 审计 + Outbox）造出来并真机跑通——**这是真资产**。但请**别让这份真资产，给尚未验证的 Agent 可靠性、ERP 自动写、反馈进化背书**。
>
> **先切一条、跑出数、每步设闸。**慢就是快。

<div align="right"><sub>本文与《数字生命体版》并存 · 内嵌 base64 SVG(<code>docs/assets/organism/build2.cjs</code>) · 立项看愿景版，评审用本版</sub></div>
`;

const out = path.resolve(__dirname, '../../20260901_企业智能落地_务实版(对抗性检验后).md');
fs.writeFileSync(out, md);
const svgDir = path.resolve(__dirname, 'svg2'); fs.mkdirSync(svgDir, { recursive: true });
for (const k in D) fs.writeFileSync(path.join(svgDir, k + '.svg'), D[k]);
console.log('written:', out, '| bytes:', fs.statSync(out).size, '| diagrams:', Object.keys(D).join(', '));
