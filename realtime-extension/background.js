// ============================================================
// Video Analyzer — Background Service Worker
//
// 职责：
//   1. 通过 Native Messaging 管理本地 Rust 后端的启停与状态查询
//   2. 代理 content script 的 HTTP 请求（proxyFetch），绕过页面 CSP
//   3. 把 content script 的分析日志导出为 JSON 文件
//
// 协议约定：Native Messaging 使用"4 字节小端长度前缀 + JSON"格式，
// 具体实现见 native_launcher.ps1。
// ============================================================

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
  if (msg.type === 'saveLog') {
    // 导出分析日志为 JSON 文件（通过 chrome.downloads 触发下载）
    const log = msg.log;
    const ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    const safeTitle = (log.title || 'untitled').replace(/[\\/:*?"<>|]/g, '_').slice(0, 40);
    const filename = `video-analysis_${safeTitle}_${ts}.json`;
    const blob = new Blob([JSON.stringify(log, null, 2)], { type: 'application/json' });
    const reader = new FileReader();
    reader.onload = () => {
      chrome.downloads.download({
        url: reader.result,
        filename,
        saveAs: false,
      }, () => {
        if (chrome.runtime.lastError) {
          console.error('[RA] 日志保存失败:', chrome.runtime.lastError.message);
        } else {
          console.log('[RA] 日志已保存:', filename);
        }
      });
    };
    reader.readAsDataURL(blob);
    return false;
  }
});
