// ============================================================
// Video Realtime Analyzer — Popup（通用版）
//
// 协议约定：后端 /analyze-frame 返回的 score 为 0~1（见 evaluator.rs），
// 界面展示时统一乘以 100 显示为百分比。决策阈值与后端一致：
//   score >= 0.72 → CLOUD（送云端）   score >= 0.38 → LOCAL（本地）
//   score < 0.38  → DROP（丢弃）
// ============================================================

const POLL_MS = 800;
const CHART_MAX_POINTS = 200;

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
const statsEl    = document.getElementById('stats');
const cntC       = document.getElementById('cntC');
const cntL       = document.getElementById('cntL');
const cntD       = document.getElementById('cntD');
const cntF       = document.getElementById('cntF');
const footerEl   = document.getElementById('footer');
const btnPopout  = document.getElementById('btnPopout');
const modeBadge  = document.getElementById('modeBadge');

let timer        = null;
let analyzing    = false;
let lastHistoryLen = 0;

const isStandalone = !!(chrome.windows && window.innerWidth > 420);

// ============================================================
// 评分仪表条颜色映射（score 为 0~1，阈值与后端一致）
// ============================================================
function scoreColor(s) {
  if (s >= THRESHOLD_CLOUD) return '#00ff88';   // 高价值：送云端 → 绿
  if (s >= THRESHOLD_LOCAL) return '#ffcc00';   // 中价值：本地处理 → 黄
  return '#ff6666';                              // 低价值：丢弃 → 红
}

// ============================================================
// Build UI（指标卡由 FEATURES 数据源驱动生成）
// ============================================================
FEATURES.forEach(ind => {
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
      if (!isStandalone) {
        chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), type: 'popup', width: 740, height: 620 });
      }
    } else {
      hintEl.textContent = resp?.error || '启动失败';
    }
  } catch (e) {
    hintEl.textContent = e.message === 'NO_TAB' ? '请先打开一个有视频播放的页面' : '通信失败，请刷新页面后重试';
  }
});

btnPopout.addEventListener('click', () => {
  chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), type: 'popup', width: 740, height: 620 });
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
  statsEl.classList.add('show');
  btnPopout.classList.add('show');
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
  statsEl.classList.remove('show');
  btnPopout.classList.remove('show');
  modeBadge.classList.remove('show', 'cloud', 'local', 'drop');
  footerEl.textContent = '点击按钮开始';
  lastHistoryLen = 0;
  ctx.clearRect(0, 0, chartEl.width, chartEl.height);
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

  // 图表
  if (hist.length !== lastHistoryLen) {
    lastHistoryLen = hist.length;
    const chartData = hist.length > CHART_MAX_POINTS ? hist.slice(-CHART_MAX_POINTS) : hist;
    drawChart(chartData);
  }

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
  const pad = { top: 16, right: 44, bottom: 30, left: 10 };
  const pw = W - pad.left - pad.right;
  const ph = H - pad.top - pad.bottom;

  const x = (i) => pad.left + (i / (N - 1)) * pw;

  // 动态上限
  const dynMax = {};
  for (const line of LINES) {
    dynMax[line.key] = Math.max(NORM_MAX[line.key], ...hist.map(h => h.features[line.key] || 0));
  }

  // ========== 决策色带背景 ==========
  for (let i = 0; i < N - 1; i++) {
    const a = (hist[i].evaluation.action || '').toUpperCase();
    const alpha = 0.08;
    if (a === 'CLOUD')      ctx.fillStyle = `rgba(0,255,136,${alpha})`;
    else if (a === 'DROP')  ctx.fillStyle = `rgba(255,102,102,${alpha})`;
    else                     ctx.fillStyle = `rgba(255,204,0,${alpha})`;
    const x1 = x(i);
    const x2 = x(i + 1);
    ctx.fillRect(x1, pad.top, x2 - x1 + 1, ph);
  }

  // ========== 网格 + Y轴标注 ==========
  ctx.strokeStyle = '#21262d';
  ctx.lineWidth = 0.5;
  ctx.fillStyle = '#8b949e';
  ctx.font = `${9 * dpr}px "Segoe UI", sans-serif`;
  ctx.textAlign = 'right';
  for (let i = 0; i <= 4; i++) {
    const gy = pad.top + (i / 4) * ph;
    ctx.beginPath(); ctx.moveTo(pad.left, gy); ctx.lineTo(W - pad.right, gy); ctx.stroke();
    ctx.fillText((100 - i * 25) + '%', W - pad.right + 5, gy + 3);
  }

  // ========== 折线 ==========
  for (const line of LINES) {
    ctx.beginPath();
    ctx.strokeStyle = line.color;
    ctx.lineWidth = 1.6 * dpr;
    ctx.lineJoin = 'round';
    let prevY = null;
    for (let i = 0; i < N; i++) {
      const v = hist[i].features[line.key] || 0;
      const m = dynMax[line.key] || 1;
      const y = pad.top + ph - (Math.min(v, m) / m) * ph;
      if (i === 0) ctx.moveTo(x(i), y);
      else {
        // 两段之间的点跳过突变（视频切换场景）
        if (prevY !== null && Math.abs(y - prevY) > ph * 0.8) {
          ctx.moveTo(x(i), y);
        } else {
          ctx.lineTo(x(i), y);
        }
      }
      prevY = y;
    }
    ctx.stroke();
  }

  // ========== SMA 趋势线（窗口=10，虚线半透明） ==========
  const SMA_WIN = 10;
  for (const line of LINES) {
    ctx.beginPath();
    ctx.strokeStyle = line.color;
    ctx.lineWidth = 1.0 * dpr;
    ctx.setLineDash([4 * dpr, 4 * dpr]);
    ctx.globalAlpha = 0.4;
    ctx.lineJoin = 'round';
    let firstPoint = true;
    for (let i = 0; i < N; i++) {
      const start = Math.max(0, i - SMA_WIN + 1);
      let sum = 0, count = 0;
      for (let j = start; j <= i; j++) {
        sum += hist[j].features[line.key] || 0;
        count++;
      }
      const avg = count ? sum / count : 0;
      const m = dynMax[line.key] || 1;
      const y = pad.top + ph - (Math.min(avg, m) / m) * ph;
      if (firstPoint) { ctx.moveTo(x(i), y); firstPoint = false; }
      else ctx.lineTo(x(i), y);
    }
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;
  }

  // ========== 评分虚线叠加（score 0~1 → 0~100% 坐标系，与网格一致） ==========
  ctx.beginPath();
  ctx.strokeStyle = '#ffffff';
  ctx.lineWidth = 1.2 * dpr;
  ctx.setLineDash([5 * dpr, 4 * dpr]);
  ctx.globalAlpha = 0.55;
  for (let i = 0; i < N; i++) {
    const v = (hist[i].evaluation.score || 0) * 100;
    const y = pad.top + ph - v * ph / 100;
    if (i === 0) ctx.moveTo(x(i), y); else ctx.lineTo(x(i), y);
  }
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.globalAlpha = 1;

  // ========== 图例 ==========
  const legendY = H - 5;
  let lx = pad.left;
  ctx.font = `${10 * dpr}px "Segoe UI", sans-serif`;
  for (const line of LINES) {
    const v = hist[hist.length - 1].features[line.key] ?? 0;
    const text = `${line.name} ${v.toFixed(1)}`;
    const tw = ctx.measureText(text).width + 18 * dpr;
    if (lx + tw > W - pad.right) break;
    ctx.fillStyle = line.color;
    ctx.beginPath(); ctx.arc(lx + 5 * dpr, legendY - 3 * dpr, 3.5 * dpr, 0, Math.PI * 2); ctx.fill();
    ctx.fillStyle = '#c9d1d9';
    ctx.textAlign = 'left';
    ctx.fillText(text, lx + 12 * dpr, legendY);
    lx += tw;
  }

  // 评分图例
  const scText = '评分 ' + (((hist[hist.length - 1].evaluation.score || 0) * 100).toFixed(0));
  const scTw = ctx.measureText(scText).width + 16 * dpr;
  if (lx + scTw <= W - pad.right) {
    ctx.strokeStyle = '#ffffff88';
    ctx.lineWidth = 1.2 * dpr;
    ctx.setLineDash([5 * dpr, 3 * dpr]);
    ctx.beginPath(); ctx.moveTo(lx + 2, legendY - 3 * dpr); ctx.lineTo(lx + 13 * dpr, legendY - 3 * dpr); ctx.stroke();
    ctx.setLineDash([]);
    ctx.fillStyle = '#c9d1d9';
    ctx.textAlign = 'left';
    ctx.fillText(scText, lx + 14 * dpr, legendY);
  }
}

// ============================================================
// Auto-detect on load
// ============================================================
(async () => {
  if (isStandalone) {
    document.body.style.width = '720px';
    document.body.style.minHeight = '560px';
  }
  try {
    const resp = await sendToContent({ type: 'getState' });
    if (resp && resp.running) {
      setUIRunning();
      startPolling();
    }
  } catch (_) {}
})();
