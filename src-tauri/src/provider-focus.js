// Runs at document start in the provider's top-level page only. It puts the
// cursor in the message box, and remembers whether the user has typed anything
// so Kreide can unload untouched pages when hidden. Only that yes/no is
// reported; no IPC access, and nothing on the page is read or sent to Kreide.
(() => {
  if (window.top !== window || !['chatgpt.com', 'claude.ai'].includes(location.hostname)) return;
  const composerSelector = '#prompt-textarea, textarea[name="prompt-textarea"], [contenteditable="true"], textarea[placeholder*="Message"], textarea[placeholder*="Ask"], textarea[placeholder*="Reply"]';
  const visible = el => !!el && !el.closest('[hidden], [aria-hidden="true"]') && el.getClientRects().length > 0;
  function focus() {
    const el = [...document.querySelectorAll(composerSelector)].find(visible);
    if (!el) return false;
    el.focus();
    if (el.isContentEditable) { const range = document.createRange(); range.selectNodeContents(el); range.collapse(false); const sel = getSelection(); sel?.removeAllRanges(); sel?.addRange(range); }
    return true;
  }
  let used = false;
  for (const type of ['input', 'paste', 'drop']) window.addEventListener(type, () => { used = true; }, { capture: true, passive: true });
  Object.defineProperty(window, '__kreide', { value: Object.freeze({ focus, used: () => used }), writable: false, configurable: false });
  // The composer mounts after the page's scripts run: focus it once when it appears.
  document.addEventListener('DOMContentLoaded', () => {
    if (focus()) return;
    const observer = new MutationObserver(() => { if (focus()) observer.disconnect(); });
    observer.observe(document.body, { childList: true, subtree: true });
    setTimeout(() => observer.disconnect(), 20000);
  });
})();
