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
  return dom.window;
}
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

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
