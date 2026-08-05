// ============================================================
// Video Analyzer — Background Service Worker
//
// 职责：
//   1. 通过 Native Messaging 管理本地 Rust 后端的启停与状态查询
//   2. 代理 content script 的 HTTP 请求（proxyFetch），绕过页面 CSP
//   3. 分析记录管理：存储/下载/删除（chrome.storage.local，最多保存 20 条）
//
// 协议约定：Native Messaging 使用"4 字节小端长度前缀 + JSON"格式，
// 具体实现见 native_launcher.ps1。
// ============================================================

// 启用侧边栏模式（Chrome 114+）；不支持时自动回退到传统弹窗 popup
try {
  chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true });
} catch(_) {
  chrome.action.setPopup({ popup: 'popup.html' });
}

const NATIVE_HOST = 'com.video.analyzer';

/** 向 Native Host 发送一条消息，返回 Promise 化的响应 */
function sendNative(msg) {
  return new Promise((resolve, reject) => {
    chrome.runtime.sendNativeMessage(NATIVE_HOST, msg, (resp) => {
      if (chrome.runtime.lastError) {
        reject(new Error(chrome.runtime.lastError.message));
      } else {
        resolve(resp);
      }
    });
  });
}

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg.type === 'serviceStart') {
    sendNative({ action: 'start' })
      .then(res => sendResponse(res))
      .catch(err => sendResponse({ status: 'error', message: err.message }));
    return true;
  }
  if (msg.type === 'serviceStop') {
    sendNative({ action: 'stop' })
      .then(res => sendResponse(res))
      .catch(err => sendResponse({ status: 'error', message: err.message }));
    return true;
  }
  if (msg.type === 'serviceStatus') {
    sendNative({ action: 'status' })
      .then(res => sendResponse(res))
      .catch(err => sendResponse({ status: 'error', message: err.message }));
    return true;
  }
  if (msg.type === 'proxyFetch') {
    // 代理 fetch：绕过页面 CSP，content script → background → Rust
    fetch('http://127.0.0.1:5000/analyze-frame', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(msg.payload),
    })
      .then(async r => {
        const data = await r.json();
        sendResponse({ ok: true, data });
      })
      .catch(err => sendResponse({ ok: false, error: err.message }));
    return true;
  }
  if (msg.type === 'setRegion') {
    // 切换关注区间 → POST /set-region
    fetch('http://127.0.0.1:5000/set-region', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ region: msg.region }),
    })
      .then(async r => { sendResponse(await r.json()); })
      .catch(err => sendResponse({ ok: false, error: err.message }));
    return true;
  }
  if (msg.type === 'saveLog') {
    // 存储到 chrome.storage.local（最多保留 20 条），不立刻下载
    (async () => {
      const log = msg.log;
      const record = {
        id: Date.now(),
        timestamp: new Date().toISOString(),
        url: log.url,
        title: log.title,
        startTime: log.startTime,
        endTime: log.endTime,
        totalFrames: log.totalFrames,
        records: log.records
      };
      try {
        const { analysisLogs } = await chrome.storage.local.get('analysisLogs');
        const logs = analysisLogs || [];
        logs.push(record);
        const MAX_LOGS = 20;
        while (logs.length > MAX_LOGS) logs.shift();
        await chrome.storage.local.set({ analysisLogs: logs });
        sendResponse({ ok: true, id: record.id });
      } catch (e) {
        sendResponse({ ok: false, error: e.message });
      }
    })();
    return true;
  }
  if (msg.type === 'getLogs') {
    chrome.storage.local.get('analysisLogs').then(({ analysisLogs }) => {
      const logs = (analysisLogs || []).map(l => ({
        id: l.id, timestamp: l.timestamp, url: l.url, title: l.title,
        startTime: l.startTime, endTime: l.endTime, totalFrames: l.totalFrames,
        cloudCount: l.records.filter(r => r.evaluation.action === 'CLOUD').length,
        localCount: l.records.filter(r => r.evaluation.action === 'LOCAL').length,
        dropCount:  l.records.filter(r => r.evaluation.action === 'DROP').length,
      }));
      logs.sort((a, b) => b.id - a.id);
      sendResponse({ ok: true, logs });
    });
    return true;
  }
  if (msg.type === 'downloadLog') {
    (async () => {
      const { analysisLogs } = await chrome.storage.local.get('analysisLogs');
      const rec = (analysisLogs || []).find(l => l.id === msg.id);
      if (!rec) { sendResponse({ ok: false, error: '记录不存在' }); return; }
      const ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
      const safeTitle = (rec.title || 'untitled').replace(/[\\/:*?"<>|]/g, '_').slice(0, 40);
      const filename = `video-analysis_${safeTitle}_${ts}.json`;
      const blob = new Blob([JSON.stringify(rec, null, 2)], { type: 'application/json' });
      const reader = new FileReader();
      reader.onload = () => {
        chrome.downloads.download({ url: reader.result, filename, saveAs: true }, () => {
          if (chrome.runtime.lastError) console.error('[RA] 下载失败:', chrome.runtime.lastError.message);
        });
      };
      reader.readAsDataURL(blob);
      sendResponse({ ok: true });
    })();
    return true;
  }
  if (msg.type === 'deleteLog') {
    (async () => {
      const { analysisLogs } = await chrome.storage.local.get('analysisLogs');
      const logs = (analysisLogs || []).filter(l => l.id !== msg.id);
      await chrome.storage.local.set({ analysisLogs: logs });
      sendResponse({ ok: true });
    })();
    return true;
  }
  if (msg.type === 'deleteAllLogs') {
    chrome.storage.local.set({ analysisLogs: [] }).then(() => sendResponse({ ok: true }));
    return true;
  }
});