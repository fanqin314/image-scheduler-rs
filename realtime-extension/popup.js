// ============================================================
// Video Realtime Analyzer — Popup（通用版）
//
// 协议约定：后端 /analyze-frame 返回的 score 为 0~1（见 evaluator.rs），
// 界面展示时统一乘以 100 显示为百分比。决策阈值与后端一致：
//   score >= 0.72 → CLOUD（送云端）   score >= 0.38 → LOCAL（本地）
//   score < 0.38  → DROP（丢弃）
// ============================================================

const POLL_MS = 800;
const DISPLAY_WINDOW = 150;  // 图表最多显示最近 150 帧（滑动窗口）

// 指标与图表共用同一份数据源（key 与后端 FeatureResponse 字段一致）
// norm(v) 将原始特征值映射到 0~1，用于指标条宽度；max 为图表动态上限下限
const FEATURES = [
  { key: 'entropy',         name: '熵值',       color: '#00ff88', max: 8.0,  dec: 2, norm: v => v / 8.0 },
  { key: 'edge_ratio',      name: '边缘比',     color: '#58a6ff', max: 1.0,  dec: 3, norm: v => v / 1.0 },
  { key: 'motion',          name: '运动量',     color: '#ffaa44', max: 30.0, dec: 2, norm: v => v / 30.0 },
  { key: 'local_variance',  name: '局部方差',   color: '#bc8cff', max: 0.05, dec: 4, norm: v => v / (v + 1) },
  { key: 'brightness',      name: '亮度',       color: '#ffdd44', max: 1.0,  dec: 3, norm: v => v / 1.0 },
  { key: 'color_richness',  name: '色彩丰富度', color: '#44ddff', max: 1.0,  dec: 3, norm: v => v / 1.0 },
];
// 图表折线直接复用 FEATURES（去重保证指标卡与图表口径一致）
const LINES = FEATURES.map(f => ({ key: f.key, color: f.color, name: f.name }));
// 图表动态上限下限
const NORM_MAX = Object.fromEntries(FEATURES.map(f => [f.key, f.max]));

// 决策阈值（与后端 config.rs 保持一致）
const THRESHOLD_CLOUD = 0.72;
const THRESHOLD_LOCAL = 0.38;

// DOM refs
const btnStart   = document.getElementById('btnStart');
const hintEl     = document.getElementById('hint');
const indWrap    = document.getElementById('indicators');
const scoreWrap  = document.getElementById('scoreWrap');
const scoreBar   = document.getElementById('scoreBar');
const scoreVal   = document.getElementById('scoreVal');
const chartWrap  = document.getElementById('chartWrap');
const chartEl    = document.getElementById('chart');
const ctx        = chartEl.getContext('2d');
const chartTip   = document.getElementById('chartTooltip');
const legendBar  = document.getElementById('legendBar');
const statsEl    = document.getElementById('stats');
const cntC       = document.getElementById('cntC');
const cntL       = document.getElementById('cntL');
const cntD       = document.getElementById('cntD');
const cntF       = document.getElementById('cntF');
const summaryCard = document.getElementById('summaryCard');
const summaryClose = document.getElementById('summaryClose');
const sumFrames  = document.getElementById('sumFrames');
const sumAvg     = document.getElementById('sumAvg');
const sumMax     = document.getElementById('sumMax');
const sumMin     = document.getElementById('sumMin');
const sumCloud   = document.getElementById('sumCloud');
const sumLocal   = document.getElementById('sumLocal');
const sumDrop    = document.getElementById('sumDrop');
const sumFps     = document.getElementById('sumFps');
const historyPanel = document.getElementById('historyPanel');
const historyList  = document.getElementById('historyList');
const historyClear = document.getElementById('historyClear');
const footerEl   = document.getElementById('footer');
const modeBadge  = document.getElementById('modeBadge');

let timer        = null;
let analyzing    = false;
let lastHistoryLen = 0;

// ---- 图表状态：缩放范围 + 最近绘制数据（供 tooltip/缩放复用） ----
let chartHist = [];        // 最近一次绘制用的历史数据
let zoomRange = null;      // { start, end } 可见索引范围（含 start 不含 end），null=全部
let lastLayout = null;     // 最近一次绘制的坐标布局 { pad, pw, ph, start, end, dpr }
let tooltipIdx = -1;       // 当前 tooltip 指向的数据索引
let dragNav = false;        // 正在拖拽导航条
let dragAnchor = null;      // 拖拽锚点 { start, mx }

// ============================================================
// 评分仪表条颜色映射（score 为 0~1，阈值与后端一致）
// ============================================================
function scoreColor(s) {
  if (s >= THRESHOLD_CLOUD) return '#00ff88';   // 高价值：送云端 → 绿
  if (s >= THRESHOLD_LOCAL) return '#ffcc00';   // 中价值：本地处理 → 黄
  return '#ff6666';                              // 低价值：丢弃 → 红
}

// ============================================================
// Build UI（指标卡已在 popup.html 预渲染，此处仅兜底补建）
// ============================================================
FEATURES.forEach(ind => {
  if (document.getElementById('ind-' + ind.key)) return; // HTML 已有则跳过
  const div = document.createElement('div');
  div.className = 'ind';
  div.id = 'ind-' + ind.key;
  div.innerHTML = `<div class="label">${ind.name}</div><div class="val">--</div><div class="bar-wrap"><div class="bar" style="width:0%;background:${ind.color}"></div></div>`;
  indWrap.appendChild(div);
});

// ============================================================
// Tab discovery
// ============================================================
async function findVideoTab() {
  const tabs = await chrome.tabs.query({});
  const httpTabs = tabs.filter(t => t.url && /^https?:\/\//.test(t.url));
  for (const tab of httpTabs) {
    try {
      const resp = await chrome.tabs.sendMessage(tab.id, { type: 'getState' });
      if (resp && resp.connected) return tab;
    } catch (_) {}
  }
  const [active] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (active && active.url && /^https?:\/\//.test(active.url)) return active;
  return httpTabs[0] || null;
}

// ============================================================
// Messaging
// ============================================================
async function sendToContent(msg, retry = true) {
  const tab = await findVideoTab();
  if (!tab) throw new Error('NO_TAB');
  try {
    return await chrome.tabs.sendMessage(tab.id, msg);
  } catch (_) {
    if (!retry) throw _;
    try {
      await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: ['shadow-hook.js', 'content.js'] });
    } catch (_e) { throw _; }
    return sendToContent(msg, false);
  }
}

// ============================================================
// Start / Stop
// ============================================================
btnStart.addEventListener('click', async () => {
  if (analyzing) {
    try { await sendToContent({ type: 'stopAnalysis' }); } catch (_) {}
    chrome.runtime.sendMessage({ type: 'serviceStop' });
    stopPolling();
    setUIStopped();
    return;
  }

  try {
    hintEl.textContent = '正在启动后端服务…';
    let backendReady = false;
    // 先检查后端是否已在运行，避免重复启动
    try {
      const st0 = await chrome.runtime.sendMessage({ type: 'serviceStatus' });
      if (st0 && st0.port_ready) backendReady = true;
    } catch (_) {}
    if (!backendReady) {
      const svcResp = await chrome.runtime.sendMessage({ type: 'serviceStart' });
      if (svcResp && svcResp.status === 'error') {
        hintEl.textContent = '后端启动失败: ' + svcResp.message;
        return;
      }
      for (let i = 0; i < 25; i++) {
        const st = await chrome.runtime.sendMessage({ type: 'serviceStatus' });
        if (st && st.port_ready) break;
        await new Promise(r => setTimeout(r, 200));
      }
    }
  } catch (e) {
    hintEl.textContent = '后端通信失败: ' + e.message;
    return;
  }

  try {
    const resp = await sendToContent({ type: 'startAnalysis' });
    if (resp && resp.started) {
      hintEl.textContent = '';
      setUIRunning();
      startPolling();
      // 侧边栏模式：界面直接显示在 popup 中，无需弹出独立窗口
    } else {
      hintEl.textContent = resp?.error || '启动失败';
    }
  } catch (e) {
    hintEl.textContent = e.message === 'NO_TAB' ? '请先打开一个有视频播放的页面' : '通信失败，请刷新页面后重试';
  }
});

// ============================================================
// UI State
// ============================================================
function setUIRunning() {
  analyzing = true;
  btnStart.textContent = '停止分析';
  btnStart.className = 'running';
  hintEl.textContent = '';
  indWrap.classList.add('show');
  scoreWrap.classList.add('show');
  chartWrap.classList.add('show');
  legendBar.classList.add('show');
  statsEl.classList.add('show');
  summaryCard.classList.add('show'); // 每次启动都重新展示摘要
  footerEl.textContent = '等待首帧…';
}

function setUIStopped() {
  analyzing = false;
  btnStart.textContent = '开始分析';
  btnStart.className = '';
  hintEl.textContent = '';
  indWrap.classList.remove('show');
  scoreWrap.classList.remove('show');
  chartWrap.classList.remove('show');
  legendBar.classList.remove('show');
  statsEl.classList.remove('show');
  summaryCard.classList.remove('show');
  modeBadge.classList.remove('show', 'cloud', 'local', 'drop');
  footerEl.textContent = '点击按钮开始';
  lastHistoryLen = 0;
  chartHist = [];
  zoomRange = null;
  lastLayout = null;
  tooltipIdx = -1;
  dragNav = false;
  dragAnchor = null;
  hideTooltip();
  ctx.clearRect(0, 0, chartEl.width, chartEl.height);
  loadHistory();
}

// ============================================================
// Polling
// ============================================================
function startPolling() { stopPolling(); timer = setInterval(poll, POLL_MS); }
function stopPolling()  { if (timer) { clearInterval(timer); timer = null; } }

async function poll() {
  try {
    const resp = await sendToContent({ type: 'getState' });
    if (!resp || !resp.connected) { footerEl.textContent = '未检测到视频'; return; }
    footerEl.textContent = '';
    render(resp);
  } catch (_) { footerEl.textContent = '连接断开，请刷新页面'; }
}

// ============================================================
// Render
// ============================================================
function render(state) {
  const hist = state.history;
  const latest = hist.length ? hist[hist.length - 1] : null;

  // 指标卡（宽度 = norm(v) × 100，任何值域都能正确显示）
  for (const ind of FEATURES) {
    const el = document.getElementById('ind-' + ind.key);
    if (!el) continue;
    const v = latest ? (latest.features[ind.key] ?? 0) : 0;
    el.querySelector('.val').textContent = v.toFixed(ind.dec || 2);
    el.querySelector('.bar').style.width = Math.min(100, ind.norm(v) * 100) + '%';
  }

  // 评分仪表条（score 0~1 → 百分比显示）
  if (latest) {
    const s = latest.evaluation.score || 0;
    const color = scoreColor(s);
    scoreBar.style.width = Math.min(100, s * 100) + '%';
    scoreBar.style.background = color;
    scoreVal.textContent = (s * 100).toFixed(0);
    scoreVal.style.color = color;
  }

  // 模式徽章
  if (latest) {
    const action = (latest.evaluation.action || '').toUpperCase();
    modeBadge.textContent = action;
    modeBadge.className = 'mode-badge show ' + (action === 'CLOUD' ? 'cloud' : action === 'DROP' ? 'drop' : 'local');
  }

  // 统计
  const cc = hist.filter(h => h.evaluation.action === 'CLOUD').length;
  const ll = hist.filter(h => h.evaluation.action === 'LOCAL').length;
  const dd = hist.filter(h => h.evaluation.action === 'DROP').length;
  const total = cc + ll + dd || 1;
  cntC.textContent = cc;
  cntL.textContent = ll;
  cntD.textContent = dd;
  cntC.parentElement.querySelector('.pct-bar').style.width = (cc / total * 100).toFixed(1) + '%';
  cntL.parentElement.querySelector('.pct-bar').style.width = (ll / total * 100).toFixed(1) + '%';
  cntD.parentElement.querySelector('.pct-bar').style.width = (dd / total * 100).toFixed(1) + '%';

  // FPS: 最近 3 秒内帧数 / 3
  if (hist.length >= 2) {
    const now = hist[hist.length - 1].ts;
    const threeSecAgo = now - 3000;
    const recent = hist.filter(h => h.ts >= threeSecAgo);
    const fps = recent.length >= 2 ? (recent.length / ((now - recent[0].ts) / 1000)) : 0;
    cntF.textContent = fps.toFixed(1);
    cntF.parentElement.querySelector('.pct-bar').style.width = Math.min(100, fps * 50) + '%'; // 2fps = 100%
  }

  // 图例实时值（与指标卡同源，最新帧的六维值）
  if (latest) {
    document.querySelectorAll('.legend-item').forEach(item => {
      const key = item.dataset.key;
      const feat = FEATURES.find(f => f.key === key);
      const v = latest.features[key] ?? 0;
      item.querySelector('.legend-val').textContent = v.toFixed(feat ? feat.dec : 2);
    });
  }

  // 分析摘要（评分以 0~100 展示）
  if (hist.length > 0) {
    const scores = hist.map(h => h.evaluation.score * 100);
    const avg = scores.reduce((a, b) => a + b, 0) / scores.length;
    sumFrames.textContent = hist.length;
    sumAvg.textContent = avg.toFixed(0);
    sumMax.textContent = Math.max(...scores).toFixed(0);
    sumMin.textContent = Math.min(...scores).toFixed(0);
    sumCloud.textContent = cc;
    sumLocal.textContent = ll;
    sumDrop.textContent = dd;
    sumFps.textContent = cntF.textContent;
  }

  // 图表（每轮都重绘以支持滚动 + 滑动窗口，800ms 间隔开销可接受）
  chartHist = hist.length > DISPLAY_WINDOW ? hist.slice(-DISPLAY_WINDOW) : hist;
  drawChart(chartHist);

  if (latest) {
    footerEl.textContent = `帧#${latest.seq} | 评分 ${(latest.evaluation.score * 100).toFixed(0)} | ${latest.evaluation.action}`;
  }
}

// ============================================================
// Chart（折线定义复用顶部 FEATURES，见文件头常量区）
// ============================================================

function drawChart(hist) {
  const dpr = window.devicePixelRatio || 1;
  const W = chartEl.width  = chartEl.clientWidth  * dpr;
  const H = chartEl.height = chartEl.clientHeight * dpr;
  ctx.clearRect(0, 0, W, H);
  if (hist.length < 2) return;

  const N = hist.length;
  const pad = { top: 4, right: 6, bottom: 4, left: 4 };
  const pw = W - pad.left - pad.right;
  const ph = H - pad.top - pad.bottom;

  // 可见范围（zoom）
  let start = 0, end = N;
  if (zoomRange) {
    start = Math.max(0, zoomRange.start);
    end = Math.min(N, zoomRange.end);
    if (end - start < 2) { start = 0; end = N; }
  }
  const visN = end - start;
  const x = (i) => pad.left + ((i - start) / Math.max(visN - 1, 1)) * pw;

  // 动态上限
  const dynMax = {};
  for (const line of LINES) {
    dynMax[line.key] = Math.max(NORM_MAX[line.key], ...hist.map(h => h.features[line.key] || 0));
  }

  // ========== 分面图：6 指标行 + 评分行 + 决策带 ==========
  const ROWS = 6;                     // 指标行数
  const nRows = ROWS + 3;             // +评分 +决策 +导航条
  const rowGap = 2 * dpr;
  const rowH = (ph - (nRows - 1) * rowGap) / nRows;
  const labelW = 48 * dpr;            // 左侧标签区宽度
  const SMA_WIN = 12;

  // ---- 指标行 ----
  for (let r = 0; r < ROWS; r++) {
    const line = LINES[r];
    const rowY = pad.top + r * (rowH + rowGap);
    const m = dynMax[line.key] || 1;
    const cy = rowY + rowH * 0.5;

    // 颜色圆点 + 名称
    ctx.fillStyle = line.color;
    ctx.beginPath();
    ctx.arc(pad.left + 8 * dpr, cy, 3 * dpr, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = '#8b949e';
    ctx.font = `${9 * dpr}px "Segoe UI", sans-serif`;
    ctx.textAlign = 'left';
    ctx.fillText(line.name, pad.left + 16 * dpr, cy + 3 * dpr);

    // SMA 趋势线（虚线半透明，先画在底层）
    ctx.beginPath();
    ctx.strokeStyle = line.color;
    ctx.lineWidth = 1.0 * dpr;
    ctx.setLineDash([3 * dpr, 3 * dpr]);
    ctx.globalAlpha = 0.4;
    ctx.lineJoin = 'round';
    let firstSMA = true;
    for (let i = start; i < end; i++) {
      const s = Math.max(0, i - SMA_WIN + 1);
      let sum = 0, count = 0;
      for (let j = s; j <= i; j++) { sum += hist[j].features[line.key] || 0; count++; }
      const avg = count ? sum / count : 0;
      const y = rowY + rowH - 2 * dpr - (Math.min(avg, m) / m) * (rowH - 4 * dpr);
      if (firstSMA) { ctx.moveTo(x(i), y); firstSMA = false; }
      else ctx.lineTo(x(i), y);
    }
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    // 原始折线（细线叠加在 SMA 上方）
    ctx.beginPath();
    ctx.strokeStyle = line.color;
    ctx.lineWidth = 0.7 * dpr;
    ctx.globalAlpha = 0.65;
    ctx.lineJoin = 'round';
    for (let i = start; i < end; i++) {
      const v = hist[i].features[line.key] || 0;
      const y = rowY + rowH - 2 * dpr - (Math.min(v, m) / m) * (rowH - 4 * dpr);
      if (i === start) ctx.moveTo(x(i), y);
      else ctx.lineTo(x(i), y);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;

    // 右侧最大范围注记
    ctx.fillStyle = '#484f58';
    ctx.textAlign = 'right';
    ctx.font = `${7 * dpr}px "Segoe UI", sans-serif`;
    ctx.fillText(r > 1 ? m.toFixed(1) : m.toFixed(2), W - pad.right - 2, cy + 3 * dpr);
  }

  // ---- 评分行 ----
  const scoreY = pad.top + ROWS * (rowH + rowGap);
  const scoreH = rowH;
  const scoreCy = scoreY + scoreH * 0.5;
  ctx.fillStyle = '#484f58';
  ctx.font = `${8 * dpr}px "Segoe UI", sans-serif`;
  ctx.textAlign = 'left';
  ctx.fillText('评分', pad.left + 4, scoreCy + 2 * dpr);
  ctx.beginPath();
  ctx.strokeStyle = '#ffffff';
  ctx.lineWidth = 1.2 * dpr;
  ctx.globalAlpha = 0.55;
  for (let i = start; i < end; i++) {
    const v = (hist[i].evaluation.score || 0) * 100;
    const y = scoreY + scoreH - 2 * dpr - v * (scoreH - 4 * dpr) / 100;
    if (i === start) ctx.moveTo(x(i), y); else ctx.lineTo(x(i), y);
  }
  ctx.stroke();
  ctx.globalAlpha = 1;
  // 右侧标注阈值线
  ctx.fillStyle = '#484f58';
  ctx.font = `${7 * dpr}px "Segoe UI", sans-serif`;
  ctx.textAlign = 'right';
  ctx.fillText('100', W - pad.right - 2, scoreCy + 3 * dpr);

  // ---- 决策色带行 ----
  const decY = pad.top + (ROWS + 1) * (rowH + rowGap);
  const decH = rowH;
  for (let i = start; i < end - 1; i++) {
    const a = (hist[i].evaluation.action || '').toUpperCase();
    if (a === 'CLOUD') ctx.fillStyle = 'rgba(0,255,136,0.35)';
    else if (a === 'DROP') ctx.fillStyle = 'rgba(255,102,102,0.35)';
    else ctx.fillStyle = 'rgba(255,204,0,0.35)';
    const x1 = x(i);
    const x2 = x(i + 1);
    ctx.fillRect(x1, decY, Math.max(x2 - x1 + 1, 2), decH);
  }
  const decCy = decY + decH * 0.5;
  ctx.fillStyle = '#8b949e';
  ctx.font = `${8 * dpr}px "Segoe UI", sans-serif`;
  ctx.textAlign = 'center';
  ctx.fillText('决策带', pad.left + pw * 0.12, decCy + 2 * dpr);

  // ---- 导航条（拖拽平移查看不同时间段） ----
  const navY = pad.top + (ROWS + 2) * (rowH + rowGap);
  const navH = rowH * 0.55;
  // 全部帧的决策色带缩略图
  for (let i = 0; i < N - 1; i++) {
    const a = (hist[i].evaluation.action || '').toUpperCase();
    if (a === 'CLOUD') ctx.fillStyle = 'rgba(0,255,136,0.25)';
    else if (a === 'DROP') ctx.fillStyle = 'rgba(255,102,102,0.25)';
    else ctx.fillStyle = 'rgba(255,204,0,0.25)';
    ctx.fillRect(pad.left + (i / (N - 1)) * pw, navY, pw / Math.max(N - 1, 1) + 1, navH);
  }
  // 当前可见范围白框
  const vx1 = pad.left + (start / Math.max(N - 1, 1)) * pw;
  const vx2 = pad.left + ((end - 1) / Math.max(N - 1, 1)) * pw;
  ctx.strokeStyle = '#ffffff';
  ctx.lineWidth = 1.2 * dpr;
  ctx.strokeRect(vx1, navY, vx2 - vx1, navH);
  // 拖拽手柄
  ctx.fillStyle = 'rgba(255,255,255,0.15)';
  ctx.fillRect(vx1, navY, vx2 - vx1, navH);
  const navCy = navY + navH * 0.5;
  ctx.fillStyle = '#484f58';
  ctx.font = `${7 * dpr}px "Segoe UI", sans-serif`;
  ctx.textAlign = 'left';
  ctx.fillText('◁ 拖拽平移', pad.left + 4, navCy + 2 * dpr);

  // ========== 记录布局 ==========
  lastLayout = { pad, pw, ph, start, end, dpr, W, H, rowH, labelW, ROWS, nRows, rowGap, navY, navH };

  // 缩放提示
  if (zoomRange) {
    ctx.fillStyle = 'rgba(255,255,255,0.4)';
    ctx.font = `${9 * dpr}px "Segoe UI", sans-serif`;
    ctx.textAlign = 'left';
    ctx.fillText(`缩放 ${start + 1}-${end} / ${N} · 双击重置`, pad.left + 4, pad.top + 12);
  }
}

// ============================================================
// Chart 交互：滚轮缩放 / 双击重置 / 悬停提示
// ============================================================

/** 隐藏图表悬停提示 */
function hideTooltip() {
  chartTip.style.display = 'none';
  tooltipIdx = -1;
}

/** 在 (mx, my)（相对 chart-wrap 的坐标）显示第 idx 帧的详情 */
function showTooltipAt(idx, mx, my) {
  const h = chartHist[idx];
  if (!h) return;
  const rows = FEATURES.map(f =>
    `<div class="tt-row"><span>${f.name}</span><span class="tt-val">${(h.features[f.key] ?? 0).toFixed(f.dec || 2)}</span></div>`
  ).join('');
  chartTip.innerHTML = `
    <div class="tt-title">帧#${h.seq} · ${h.evaluation.action} · 评分 ${(h.evaluation.score * 100).toFixed(0)}</div>
    ${rows}
  `;
  const wrapRect = chartWrap.getBoundingClientRect();
  let left = mx + 14;
  if (left + chartTip.offsetWidth > wrapRect.width - 8) left = mx - chartTip.offsetWidth - 10;
  chartTip.style.left = left + 'px';
  chartTip.style.top = (my - 10) + 'px';
  chartTip.style.display = 'block';
  tooltipIdx = idx;
}

// 悬停 / 拖拽：导航条区域拖拽平移，图表区域 tooltip
chartEl.addEventListener('mousemove', e => {
  if (!lastLayout || chartHist.length < 2) return;
  const rect = chartEl.getBoundingClientRect();
  const mx = e.clientX - rect.left;
  const my = e.clientY - rect.top;
  const { pad, pw, ph, start, end, navY, navH } = lastLayout;

  // 导航条区域：拖拽平移
  if (dragNav && dragAnchor) {
    const dx = mx - dragAnchor.mx;
    const span = end - start;
    const shift = Math.round(-dx / pw * span);
    let ns = Math.max(0, Math.min(chartHist.length - span, dragAnchor.start + shift));
    zoomRange = { start: ns, end: ns + span };
    drawChart(chartHist);
    hideTooltip();
    return;
  }

  // 图表区域：tooltip
  if (mx < pad.left || mx > pad.left + pw || my < pad.top || my > pad.top + ph - navH - rowH) { // 排除导航条
    hideTooltip();
    return;
  }
  const ratio = (mx - pad.left) / pw;
  const idx = start + Math.round(ratio * (end - start - 1));
  if (idx < 0 || idx >= chartHist.length) { hideTooltip(); return; }
  showTooltipAt(idx, mx, my);
});
chartEl.addEventListener('mouseleave', () => { hideTooltip(); dragNav = false; });

// 导航条 mousedown
chartEl.addEventListener('mousedown', e => {
  if (!lastLayout || chartHist.length < 2) return;
  const rect = chartEl.getBoundingClientRect();
  const my = e.clientY - rect.top;
  const { navY, navH, start, end } = lastLayout;
  if (navY && my >= navY && my <= navY + navH) {
    dragNav = true;
    dragAnchor = { start, mx: e.clientX - rect.left };
  }
});
// 导航条 mouseup（加在 document 上而不仅是 canvas，防止拖出 canvas 后松手失效）
document.addEventListener('mouseup', () => { dragNav = false; });

// 滚轮缩放：以鼠标位置为锚点放大/缩小可见区间
chartEl.addEventListener('wheel', e => {
  if (chartHist.length < 4) return;
  e.preventDefault();
  const rect = chartEl.getBoundingClientRect();
  const pad = lastLayout ? lastLayout.pad : { left: 10, right: 44 };
  const pw = lastLayout ? lastLayout.pw : (chartEl.clientWidth - pad.left - pad.right);
  const start = lastLayout ? lastLayout.start : 0;
  const end = lastLayout ? lastLayout.end : chartHist.length;
  const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left - pad.left) / pw));
  const span = end - start;
  const newSpan = Math.round(span * (e.deltaY > 0 ? 1.25 : 0.8)); // 滚上放大，滚下缩小
  const clamped = Math.max(10, Math.min(chartHist.length, newSpan));
  let ns = Math.round(start + ratio * span - ratio * clamped);
  ns = Math.max(0, Math.min(chartHist.length - clamped, ns));
  zoomRange = { start: ns, end: ns + clamped };
  drawChart(chartHist);
  hideTooltip();
}, { passive: false });

// 双击重置缩放
chartEl.addEventListener('dblclick', () => {
  if (!zoomRange) return;
  zoomRange = null;
  drawChart(chartHist);
  hideTooltip();
});

// 摘要卡片关闭按钮
summaryClose.addEventListener('click', () => summaryCard.classList.remove('show'));

// ============================================================
// 分析历史记录：查看 / 下载 / 删除
// ============================================================
async function loadHistory() {
  try {
    const resp = await chrome.runtime.sendMessage({ type: 'getLogs' });
    if (resp && resp.ok && resp.logs.length > 0) {
      renderHistory(resp.logs);
    } else {
      historyPanel.style.display = 'none';
    }
  } catch (_) { historyPanel.style.display = 'none'; }
}

function renderHistory(logs) {
  historyPanel.style.display = 'block';
  historyList.innerHTML = logs.map(l => {
    const ts = new Date(l.startTime).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
    const tt = (l.title || l.url || '未知页面').slice(0, 28);
    return `<div class="history-item" data-id="${l.id}">
      <div class="history-info">
        <div class="hi-title" title="${l.title || l.url || ''}">${tt}</div>
        <div class="hi-meta">${ts} · ${l.totalFrames}帧 C${l.cloudCount} L${l.localCount} D${l.dropCount}</div>
      </div>
      <div class="history-actions">
        <button class="hi-btn dl" data-action="dl">下载</button>
        <button class="hi-btn del" data-action="del">删除</button>
      </div>
    </div>`;
  }).join('');
}

historyList.addEventListener('click', async e => {
  const btn = e.target.closest('[data-action]');
  if (!btn) return;
  const id = +btn.closest('.history-item').dataset.id;
  if (btn.dataset.action === 'dl') {
    await chrome.runtime.sendMessage({ type: 'downloadLog', id });
  } else if (btn.dataset.action === 'del') {
    await chrome.runtime.sendMessage({ type: 'deleteLog', id });
    loadHistory();
  }
});

historyClear.addEventListener('click', async () => {
  if (!confirm('确定要清空全部分析历史记录吗？')) return;
  await chrome.runtime.sendMessage({ type: 'deleteAllLogs' });
  historyPanel.style.display = 'none';
});

// ============================================================
// Auto-detect on load（恢复上次正在运行的分析）
// ============================================================
(async () => {
  loadHistory();
  try {
    const resp = await sendToContent({ type: 'getState' });
    if (resp && resp.running) {
      setUIRunning();
      startPolling();
    }
  } catch (_) {}
})();
