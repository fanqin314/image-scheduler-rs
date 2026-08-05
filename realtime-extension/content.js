// ============================================================
// Real-time Video Analyzer — Content Script（通用版）
//
// 职责：找到页面正在播放的视频 → 定时截帧 → 交给 background 转发
// 给 Rust 后端分析，并维护最近 HISTORY_MAX 帧的历史记录。
// 注意：HTTP 请求统一走 background 的 proxyFetch（绕过页面 CSP），
// 本文件不直接 fetch。
// ============================================================

// ----- 可调参数 -----
let THROTTLE_MS = 500;        // 采样间隔（毫秒），可通过 setFps 消息动态切换
let lastFrameTs = 0;           // 上一帧时间戳，用于 motion 归一化
const RESIZE_W = 128;
const RESIZE_H = 128;
const JPEG_QUALITY = 0.55;      // 截帧 JPEG 压缩质量（0~1，越小越省流量）
const HISTORY_MAX = 300;        // 历史记录条数上限（超出丢弃最旧）

// ----- state -----
let video = null;
let offscreen = null;
let offCtx = null;
let lastBase64 = null;
let lastFetchTime = 0;
let frameSeq = 0;
let observer = null;
let running = false;
let history = [];

// ============================================================
// 1. 帧抓取与分析
// ============================================================

/** 懒创建离屏 canvas（128x128），用于截帧缩放 */
function ensureOffscreen() {
  if (!offscreen) {
    offscreen = document.createElement('canvas');
    offscreen.width = RESIZE_W;
    offscreen.height = RESIZE_H;
    offCtx = offscreen.getContext('2d');
  }
}

/** 常规截帧：把 <video> 当前帧画到离屏 canvas 再转 JPEG Base64 */
function captureFrame() {
  ensureOffscreen();
  try {
    offCtx.drawImage(video, 0, 0, RESIZE_W, RESIZE_H);
    return offscreen.toDataURL('image/jpeg', JPEG_QUALITY);
  } catch (e) {
    console.error('[RA] 截帧失败:', e.message);
    return null;
  }
}

/** 备用截帧：跨域受限时尝试 captureStream + ImageCapture（B 站等跨域源） */
async function captureFrameStream() {
  if (!video.captureStream) return null;
  try {
    const stream = video.captureStream();
    const track = stream.getVideoTracks()[0];
    if (!track) return null;
    const bitmap = typeof ImageCapture !== 'undefined'
      ? await new ImageCapture(track).grabFrame()
      : null;
    if (bitmap) {
      ensureOffscreen();
      offCtx.drawImage(bitmap, 0, 0, RESIZE_W, RESIZE_H);
      bitmap.close();
      return offscreen.toDataURL('image/jpeg', JPEG_QUALITY);
    }
    return null;
  } catch (e2) {
    console.error('[RA] Stream 截帧也失败:', e2.message);
    return null;
  }
}

/**
 * 分析一帧：截帧 → 通过 background 转发给 Rust /analyze-frame
 * 上一帧 Base64 一并携带，后端用它计算 motion 特征。
 * 用 requestVideoFrameCallback + 节流实现"视频帧回调驱动"的采样。
 */
async function analyzeFrame(now, metadata) {
  if (!running || !video || video.paused) return;

  const curB64 = captureFrame() || await captureFrameStream();
  if (!curB64) {
    console.error('[RA] 截帧失败(跨域?)');
    if (running && video && !video.paused) {
      setTimeout(() => video.requestVideoFrameCallback(analyzeFrame), THROTTLE_MS);
    }
    return;
  }
  const payload = {
    image_base64: curB64,
    prev_image_base64: lastBase64,
  };

  try {
    const resp = await chrome.runtime.sendMessage({ type: 'proxyFetch', payload });
    if (!resp.ok) throw new Error(resp.error || 'HTTP fail');
    const data = resp.data;
    if (data.features && data.evaluation) {
      // motion 归一化：按实际时间差折算到 500ms 基准，消除帧率影响
      const now = Date.now();
      const dt = lastFrameTs ? (now - lastFrameTs) : THROTTLE_MS;
      lastFrameTs = now;
      if (data.features.motion != null) {
        data.features.motion = data.features.motion * (500 / Math.max(dt, 1));
      }
      history.push({ seq: frameSeq, ts: now, features: data.features, evaluation: data.evaluation, vis: data.visualized_image });
      if (history.length > HISTORY_MAX) history.shift();
      frameSeq++;
    }
  } catch (e) {
    console.error('[RA] Rust 离线:', e.message);
  }

  lastBase64 = curB64;
  if (running && video && !video.paused) {
    setTimeout(() => {
      if (video.requestVideoFrameCallback) {
        video.requestVideoFrameCallback(analyzeFrame);
      } else {
        requestAnimationFrame(() => analyzeFrame(0, null));
      }
    }, THROTTLE_MS);
  }
}

/** 开始循环采样 */
function startLoop() {
  if (running) return;
  running = true;
  if (video.requestVideoFrameCallback) {
    video.requestVideoFrameCallback(analyzeFrame);
  } else {
    analyzeFrame(0, null);
  }
}

/** 停止循环采样 */
function stopLoop() {
  running = false;
}

// ============================================================
// 2. 通用 Video 发现（递归 shadow DOM + iframe）
// ============================================================

/**
 * 递归收集 root 下所有 <video>：
 * 1. 直接后代中的 video
 * 2. shadow DOM 内部（配合 shadow-hook.js 强制 open 才能访问）
 * 3. iframe 内部（跨域 iframe 访问会抛异常，捕获后跳过）
 */
function collectAllVideos(root) {
  const videos = [];
  root.querySelectorAll('video').forEach(v => videos.push(v));
  root.querySelectorAll('*').forEach(el => {
    if (el.shadowRoot) {
      videos.push(...collectAllVideos(el.shadowRoot));
    }
  });
  root.querySelectorAll('iframe').forEach(iframe => {
    try {
      const doc = iframe.contentDocument || iframe.contentWindow?.document;
      if (doc) videos.push(...collectAllVideos(doc));
    } catch (_) {}
  });
  return videos;
}

/**
 * 在所有视频中挑出"正在播放的那个"：
 * 1. 优先选 正在播放 且 有画面 且 时长>1s 的
 * 2. 其次选 有画面 且 时长>1s 的
 * 3. 兜底选 分辨率最大 的
 */
function findVideo() {
  const allVideos = collectAllVideos(document);
  if (allVideos.length === 0) return null;

  for (const v of allVideos) {
    if (v.videoWidth > 0 && v.duration > 1 && !v.paused) return v;
  }
  for (const v of allVideos) {
    if (v.videoWidth > 0 && v.duration > 1) return v;
  }
  return allVideos.reduce((a, b) =>
    (a.videoWidth * a.videoHeight) > (b.videoWidth * b.videoHeight) ? a : b
  );
}

/** 向上查找视频的"可视父节点"（尺寸 ≥ 300x200 的最近祖先） */
function getVisualParent(v) {
  let node = v.parentElement || (v.getRootNode && v.getRootNode().host);
  while (node) {
    const rect = node.getBoundingClientRect && node.getBoundingClientRect();
    if (rect && rect.width >= 300 && rect.height >= 200) return node;
    node = node.parentElement || (node.getRootNode && node.getRootNode().host);
  }
  return document.body;
}

/** 绑定到目标视频：监听播放/暂停/结束，自动开始或停止采样 */
function attachToVideo(v) {
  if (video === v) return;
  video = v;

  try { video.crossOrigin = 'anonymous'; } catch (_) {}

  history = [];
  console.log('[RA] 绑定视频:', v.tagName, v.videoWidth + 'x' + v.videoHeight);

  v.addEventListener('play', startLoop);
  v.addEventListener('pause', stopLoop);
  v.addEventListener('ended', stopLoop);

  if (!v.paused) startLoop();
}

// ============================================================
// 3. MutationObserver 监听页面变化（SPA 跳转后自动重新绑定）
// ============================================================

/** 扫描当前页面并绑定找到的视频 */
function scanAndAttach() {
  const v = findVideo();
  if (v) attachToVideo(v);
}

/** 监听 DOM 变化：当前视频消失（切页/换视频）时重新扫描绑定 */
function initObserver() {
  observer = new MutationObserver(() => {
    if (!video || !document.contains(video)) {
      video = null;
      scanAndAttach();
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
}

// ============================================================
// 4. Popup 通信
// ============================================================

/** 完整启动：找视频 + 绑定 + 开启 DOM 监听，返回给 popup 的结果 */
function fullStart() {
  const v = findVideo();
  if (!v) {
    return {
      started: false,
      error: '当前页面未检测到视频播放器。请确认视频正在播放中，然后重试。',
    };
  }
  attachToVideo(v);
  if (!observer) initObserver();
  return { started: true };
}

/** 完整停止：停止采样 + 导出日志 + 解除监听与绑定 */
function fullStop() {
  stopLoop();

  // 有历史数据则导出为 JSON 日志（由 background 的 saveLog 处理下载）
  if (history.length > 0) {
    const log = {
      url: location.href,
      title: document.title,
      startTime: history[0].ts,
      endTime: history[history.length - 1].ts,
      totalFrames: history.length,
      records: history.map(h => ({
        seq: h.seq,
        ts: h.ts,
        features: h.features,
        evaluation: h.evaluation,
      })),
    };
    chrome.runtime.sendMessage({ type: 'saveLog', log }).catch(() => {});
  }

  if (observer) { observer.disconnect(); observer = null; }
  video = null;
  history = [];
}

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg.type === 'startAnalysis') {
    sendResponse(fullStart());
  } else if (msg.type === 'stopAnalysis') {
    fullStop();
    sendResponse({ stopped: true });
  } else if (msg.type === 'setFps') {
    // 切换采样间隔：250/500/1000ms
    THROTTLE_MS = msg.ms || 500;
    sendResponse({ set: true, ms: THROTTLE_MS });
  } else if (msg.type === 'getState') {
    sendResponse({
      connected: !!video,
      running,
      history,
    });
  }
  return true;
});
