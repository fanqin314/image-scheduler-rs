// ============================================================
// Shadow DOM Hook — 页面加载前注入，强制所有 shadow root 为 open
// 必须在 document_start 执行
// ============================================================

(function() {
  const script = document.createElement('script');
  script.textContent = `
    (() => {
      const _orig = Element.prototype.attachShadow;
      Element.prototype.attachShadow = function(init) {
        return _orig.call(this, Object.assign({}, init, { mode: 'open' }));
      };
      console.log('[ShadowHook] attachShadow 已劫持，所有 shadow root 强制 open');
    })();
  `;
  (document.documentElement || document).appendChild(script);
  script.remove();
})();
