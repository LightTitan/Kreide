// Runs at document start in the provider's top-level page only. It puts the
// cursor in the message box, collapses ChatGPT's sidebar on load, and remembers
// whether the user has typed anything
// so Kreide can unload untouched pages when hidden. Only that yes/no is
// reported; no IPC access, and nothing on the page is read or sent to Kreide.
(() => {
  if (window.top !== window || !['chatgpt.com', 'claude.ai'].includes(location.hostname)) return;
  const composerSelector = '#prompt-textarea, textarea[name="prompt-textarea"], [contenteditable="true"], textarea[placeholder*="Message"], textarea[placeholder*="Ask"], textarea[placeholder*="Reply"]';
  const visible = el => !!el && !el.closest('[hidden], [aria-hidden="true"]') && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden';
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
    // Use ChatGPT's own toggle so the chat expands into the available space.
    // Stop once closed (or already collapsed), allowing manual reopening.
    if (location.hostname === 'chatgpt.com') {
      const closeSelector = 'button[aria-label="Close sidebar" i], button[data-testid="close-sidebar-button"]';
      const openSelector = 'button[aria-label="Open sidebar" i], button[data-testid="open-sidebar-button"]';
      // Server-rendered controls can appear before their click handlers work.
      // Confirm the collapsed state instead of treating a click as success.
      let lastClick = -Infinity;
      const sidebarObserver = new MutationObserver(checkSidebar);
      const retry = setInterval(checkSidebar, 250);
      const timeout = setTimeout(stopSidebar, 20000);
      function stopSidebar() {
        sidebarObserver.disconnect();
        clearInterval(retry);
        clearTimeout(timeout);
        window.removeEventListener('pagehide', stopSidebar);
        window.removeEventListener('pointerdown', userToggle, true);
        window.removeEventListener('keydown', userToggle, true);
      }
      function userToggle(event) {
        if (event.target instanceof Element && event.target.closest(`${closeSelector}, ${openSelector}`)) stopSidebar();
      }
      function checkSidebar() {
        const close = [...document.querySelectorAll(closeSelector)].find(visible);
        if (!close && [...document.querySelectorAll(openSelector)].some(visible)) { stopSidebar(); return; }
        if (close && !close.disabled && Date.now() - lastClick >= 250) {
          lastClick = Date.now();
          close.click();
        }
      }
      sidebarObserver.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['aria-label', 'aria-expanded', 'data-testid', 'disabled', 'hidden', 'aria-hidden', 'class', 'style'] });
      window.addEventListener('pagehide', stopSidebar, { once: true });
      window.addEventListener('pointerdown', userToggle, true);
      window.addEventListener('keydown', userToggle, true);
      checkSidebar();
    }
    if (focus()) return;
    const observer = new MutationObserver(() => { if (focus()) observer.disconnect(); });
    observer.observe(document.body, { childList: true, subtree: true });
    setTimeout(() => observer.disconnect(), 20000);
    window.addEventListener('pagehide', () => observer.disconnect(), { once: true });
  });
})();
