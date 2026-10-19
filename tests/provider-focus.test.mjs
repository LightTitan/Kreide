import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { JSDOM } from 'jsdom';
const script = readFileSync(new URL('../src-tauri/src/provider-focus.js', import.meta.url), 'utf8');
function page(host = 'chatgpt.com', html = '') {
  const dom = new JSDOM(`<body>${html}</body>`, { url: `https://${host}/`, runScripts: 'outside-only' });
  // jsdom has no layout engine. Mark fixture elements visible unless explicitly hidden.
  dom.window.Element.prototype.getClientRects = function () { return this.closest('[hidden]') ? [] : [{ width: 10, height: 10 }]; };
  dom.window.eval(script);
  // Simulate navigation lifecycle before tearing down jsdom's document.
  const close = dom.window.close.bind(dom.window);
  dom.window.close = () => { dom.window.dispatchEvent(new dom.window.Event('pagehide')); close(); };
  return dom.window;
}
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test('collapses the authenticated Hide sidebar control after it mounts and permits manual reopening', async () => {
  const w = page('chatgpt.com', '<textarea id="prompt-textarea"></textarea><div hidden><button aria-label="Close sidebar"></button></div>');
  await tick();
  const toggle = w.document.createElement('button');
  toggle.setAttribute('aria-label', 'Hide sidebar');
  toggle.setAttribute('aria-expanded', 'true');
  let clicks = 0;
  toggle.addEventListener('click', () => {
    clicks++;
    const expanded = toggle.getAttribute('aria-expanded') === 'true';
    toggle.setAttribute('aria-expanded', String(!expanded));
    toggle.setAttribute('aria-label', expanded ? 'Show sidebar' : 'Hide sidebar');
  });
  w.document.body.append(toggle);
  await tick();
  assert.equal(toggle.getAttribute('aria-label'), 'Show sidebar');
  assert.equal(toggle.getAttribute('aria-expanded'), 'false');
  assert.equal(clicks, 1);
  toggle.dispatchEvent(new w.Event('pointerdown', { bubbles: true }));
  toggle.click();
  await tick();
  assert.equal(toggle.getAttribute('aria-label'), 'Hide sidebar');
  assert.equal(clicks, 2);
  w.close();
});
test('recognizes an initially collapsed authenticated Show sidebar control', async () => {
  const w = page('chatgpt.com', '<button aria-label="Show sidebar"></button>');
  await tick();
  const toggle = w.document.querySelector('button');
  let clicks = 0;
  toggle.addEventListener('click', () => { clicks++; });
  toggle.setAttribute('aria-label', 'Hide sidebar');
  await tick();
  assert.equal(clicks, 0);
  w.close();
});
test('honors manual keyboard operation of the authenticated toggle before it is ready', async () => {
  const w = page('chatgpt.com', '<button aria-label="Hide sidebar"></button>');
  const toggle = w.document.querySelector('button');
  let clicks = 0;
  toggle.addEventListener('click', () => { clicks++; });
  await tick();
  toggle.dispatchEvent(new w.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  const previousClicks = clicks;
  await new Promise(resolve => setTimeout(resolve, 300));
  assert.equal(clicks, previousClicks);
  w.close();
});

test('collapses ChatGPT sidebar on load without preventing manual reopening', async () => {
  const w = page('chatgpt.com', '<button aria-label="Close sidebar"></button>');
  const toggle = w.document.querySelector('button');
  let clicks = 0;
  toggle.addEventListener('click', () => {
    clicks++;
    toggle.setAttribute('aria-label', clicks % 2 ? 'Open sidebar' : 'Close sidebar');
  });
  await tick();
  assert.equal(clicks, 1);
  toggle.click();
  await tick();
  assert.equal(clicks, 2);
  assert.equal(toggle.getAttribute('aria-label'), 'Close sidebar');
  w.close();
});
test('collapses a ChatGPT sidebar mounted after the composer', async () => {
  const w = page('chatgpt.com', '<textarea id="prompt-textarea"></textarea>');
  await tick();
  const toggle = w.document.createElement('button');
  toggle.dataset.testid = 'close-sidebar-button';
  let clicks = 0;
  toggle.addEventListener('click', () => { clicks++; toggle.dataset.testid = 'open-sidebar-button'; });
  w.document.body.append(toggle);
  await tick();
  assert.equal(clicks, 1);
  w.document.body.append(w.document.createElement('div'));
  await tick();
  assert.equal(clicks, 1);
  w.close();
});
test('retries an early click until the sidebar actually collapses, then stops', async () => {
  const w = page('chatgpt.com', '<button aria-label="Close sidebar"></button>');
  const toggle = w.document.querySelector('button');
  let ready = false;
  let clicks = 0;
  toggle.addEventListener('click', () => {
    clicks++;
    if (ready) toggle.setAttribute('aria-label', 'Open sidebar');
  });
  await tick();
  assert.equal(toggle.getAttribute('aria-label'), 'Close sidebar');
  ready = true;
  const deadline = Date.now() + 1500;
  while (toggle.getAttribute('aria-label') !== 'Open sidebar' && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.equal(toggle.getAttribute('aria-label'), 'Open sidebar');
  const completedClicks = clicks;
  // Manual reopening after success must not restart the watcher.
  toggle.setAttribute('aria-label', 'Close sidebar');
  await new Promise(resolve => setTimeout(resolve, 300));
  assert.equal(clicks, completedClicks);
  w.close();
});
test('stops automation when the user operates the sidebar during startup', async () => {
  const w = page('chatgpt.com', '<button aria-label="Close sidebar"></button>');
  const toggle = w.document.querySelector('button');
  let clicks = 0;
  toggle.addEventListener('click', () => { clicks++; });
  await tick();
  toggle.dispatchEvent(new w.Event('pointerdown', { bubbles: true }));
  const previousClicks = clicks;
  await new Promise(resolve => setTimeout(resolve, 300));
  assert.equal(clicks, previousClicks);
  w.close();
});
test('leaves an initially collapsed sidebar available to open', async () => {
  const w = page('chatgpt.com', '<button aria-label="Open sidebar"></button>');
  await tick();
  const toggle = w.document.querySelector('button');
  let clicks = 0;
  toggle.addEventListener('click', () => { clicks++; });
  toggle.setAttribute('aria-label', 'Close sidebar');
  await tick();
  assert.equal(clicks, 0);
  w.close();
});
test('does not collapse Claude sidebar or click hidden ChatGPT controls', async () => {
  for (const [host, html] of [
    ['claude.ai', '<button aria-label="Close sidebar"></button>'],
    ['chatgpt.com', '<div hidden><button aria-label="Close sidebar"></button></div>'],
  ]) {
    const w = page(host, html);
    let clicks = 0;
    w.document.querySelector('button').addEventListener('click', () => { clicks++; });
    await tick();
    assert.equal(clicks, 0);
    w.close();
  }
});

test('focuses the composer on load', () => {
  const w = page('claude.ai', '<div contenteditable="true"></div>');
  w.document.dispatchEvent(new w.Event('DOMContentLoaded'));
  assert.equal(w.document.activeElement, w.document.querySelector('[contenteditable]'));
  w.close();
});
test('focuses a composer that mounts after load', async () => {
  const w = page('chatgpt.com', '');
  w.document.dispatchEvent(new w.Event('DOMContentLoaded'));
  const editor = w.document.createElement('textarea'); editor.id = 'prompt-textarea';
  w.document.body.append(editor);
  await tick();
  assert.equal(w.document.activeElement, editor);
  w.close();
});
test('ignores hidden composers', () => {
  const w = page('chatgpt.com', '<div hidden><textarea id="prompt-textarea"></textarea></div>');
  assert.equal(w.__kreide.focus(), false);
  w.close();
});
test('never blocks typing or sending', () => {
  const w = page('chatgpt.com', '<div id="prompt-textarea" contenteditable="true"></div><button aria-label="Send prompt"></button>');
  const editor = w.document.querySelector('#prompt-textarea');
  for (const type of ['beforeinput', 'paste', 'drop']) assert.equal(editor.dispatchEvent(new w.Event(type, { bubbles: true, cancelable: true })), true, type);
  assert.equal(editor.dispatchEvent(new w.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })), true);
  assert.equal(w.document.querySelector('button').dispatchEvent(new w.MouseEvent('click', { bubbles: true, cancelable: true })), true);
  w.close();
});
test('does not inject on authentication or other unrelated origins', () => {
  const w = page('accounts.google.com', '<input>');
  assert.equal(w.__kreide, undefined); w.close();
});
test('reports whether the user has typed anything', () => {
  const w = page('chatgpt.com', '<div id="prompt-textarea" contenteditable="true"></div>');
  assert.equal(w.__kreide.used(), false);
  w.document.querySelector('#prompt-textarea').dispatchEvent(new w.Event('input', { bubbles: true }));
  assert.equal(w.__kreide.used(), true);
  w.close();
  const pasted = page('claude.ai', '<div contenteditable="true"></div>');
  pasted.document.querySelector('div').dispatchEvent(new pasted.Event('paste', { bubbles: true }));
  assert.equal(pasted.__kreide.used(), true);
  pasted.close();
});
