import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createIcons, SquarePen, Settings, RotateCw, CircleAlert } from 'lucide';
import { chatgptLogo, claudeLogo } from './logos';
import './style.css';

type Provider = 'chatgpt' | 'claude';
type Preferences = { defaultProvider: Provider; alwaysOnTop: boolean; shortcutEnabled: boolean; freshAfterMinutes: number };
type Snapshot = { preferences: Preferences; activeProvider: Provider | null; loaded: Provider[]; warning: string | null };

const desktop = isTauri();
const mac = navigator.platform.includes('Mac');
const mod = mac ? '⌘' : 'Ctrl+';
const names: Record<Provider, string> = { chatgpt: 'ChatGPT', claude: 'Claude' };
const logos: Record<Provider, string> = { chatgpt: chatgptLogo, claude: claudeLogo };
const providers: Provider[] = ['chatgpt', 'claude'];
const app = document.querySelector<HTMLDivElement>('#app')!;
const icon = (name: string) => `<i data-lucide="${name}" aria-hidden="true"></i>`;
const viewport = () => ({ width: window.innerWidth, height: window.innerHeight });

let state: Snapshot = {
  preferences: { defaultProvider: 'chatgpt', alwaysOnTop: false, shortcutEnabled: true, freshAfterMinutes: 15 },
  activeProvider: desktop ? null : 'chatgpt', loaded: [], warning: null,
};
let error: string | null = null;
let confirmReset: Provider | null = null;
let toastTimer: ReturnType<typeof setTimeout>;

document.documentElement.classList.toggle('mac', mac);

function toast(message: string) {
  let el = document.querySelector<HTMLElement>('#toast');
  if (!el) { el = document.createElement('div'); el.id = 'toast'; el.setAttribute('role', 'status'); document.body.append(el); }
  el.textContent = message;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => el?.remove(), 5000);
}
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T | undefined> {
  if (!desktop) return undefined;
  try { return await invoke<T>(command, args); }
  catch (e) { toast(String(e)); return undefined; }
}

// ── Main window: a thin toolbar above the provider page ──────────────────

function renderMain() {
  const active = state.activeProvider;
  document.body.dataset.provider = active ?? '';
  app.innerHTML = `<header class="toolbar" data-tauri-drag-region>
      <nav class="switcher" role="tablist" aria-label="AI provider">${providers.map((p, i) => `<button role="tab" data-provider="${p}" aria-selected="${active === p}" title="${names[p]} (${mod}${i + 1})">${logos[p]}<span>${names[p]}</span></button>`).join('')}</nav>
      <div class="spacer" data-tauri-drag-region></div>
      <button class="icon-button" id="new-chat" title="New temporary chat (${mod}N)" aria-label="New chat">${icon('square-pen')}</button>
      <button class="icon-button" id="settings" title="Settings (${mod},)" aria-label="Settings">${icon('settings')}</button>
    </header>
    <main class="stage">${error
      ? `<div class="message">${icon('circle-alert')}<p>${escape(error)}</p><button class="button" id="retry-open">${icon('rotate-cw')} Try again</button></div>`
      : desktop ? `<div class="message"><span class="spinner large"></span><p>Opening ${active ? names[active] : 'chat'}…</p></div>`
      : `<div class="message"><p>Browser preview: run <code>npm run desktop</code> to load the chats.</p></div>`}</main>`;
  createIcons({ icons: { SquarePen, Settings, RotateCw, CircleAlert }, attrs: { 'stroke-width': 1.75 } });
  document.querySelectorAll<HTMLButtonElement>('[data-provider]').forEach(el => el.addEventListener('click', () => {
    const provider = el.dataset.provider as Provider;
    if (!desktop) { state.activeProvider = provider; renderMain(); return; }
    if (provider !== state.activeProvider) void call('select_provider', { provider });
  }));
  document.querySelector('#new-chat')?.addEventListener('click', () => { void call('start_new_chat'); });
  document.querySelector('#settings')?.addEventListener('click', () => { void call('open_settings'); });
  document.querySelector('#retry-open')?.addEventListener('click', () => { error = null; renderMain(); void boot(); });
}
async function boot() {
  try { state = await invoke<Snapshot>('boot', { viewport: viewport() }); error = null; }
  catch (e) { error = String(e); }
  renderMain();
}
async function startMain() {
  renderMain();
  if (!desktop) return;
  await listen<Snapshot>('session-changed', event => { state = event.payload; renderMain(); });
  await listen<string>('error', event => toast(event.payload));
  await boot();
  if (state.warning) toast(state.warning);
  let frame = 0;
  window.addEventListener('resize', () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => { void call('resize_provider', { viewport: viewport() }); });
  });
}

// ── Settings window ──────────────────────────────────────────────────────

/** Matches FRESH_IMMEDIATELY in lib.rs: end the chat as soon as the window hides. */
const IMMEDIATELY = 4294967295;
const freshOptions: [number, string][] = [[IMMEDIATELY, 'Immediately'], [5, 'After 5 minutes'], [15, 'After 15 minutes'], [60, 'After 1 hour'], [240, 'After 4 hours'], [0, 'Never']];
function memoryNote(minutes: number) {
  if (minutes === IMMEDIATELY) return 'Hiding Kreide ends the chat and frees its memory.';
  const time = minutes === 0 ? '' : ` for up to ${minutes < 60 ? `${minutes} min` : `${minutes / 60} ${minutes === 60 ? 'hour' : 'hours'}`}`;
  return `Your chat stays loaded${time} and uses about 500 MB of memory, even while Kreide is hidden in the menu bar.`;
}
function toggle(key: 'alwaysOnTop' | 'shortcutEnabled', label: string) {
  return `<button class="switch" role="switch" aria-label="${label}" aria-checked="${state.preferences[key]}" data-pref="${key}"><span></span></button>`;
}
function renderSettings() {
  const p = state.preferences;
  app.innerHTML = `<main class="settings">
    <section class="group">
      <div class="row"><div><strong>Open with</strong><p>Used on launch and for every fresh chat.</p></div>
        <div class="segmented">${providers.map(v => `<button data-default="${v}" aria-pressed="${p.defaultProvider === v}">${logos[v]}${names[v]}</button>`).join('')}</div></div>
      <div class="row stacked"><div class="row-main"><div><strong>Start a fresh chat</strong><p>When you come back after hiding the window.</p></div>
        <select id="fresh">${freshOptions.map(([v, l]) => `<option value="${v}" ${p.freshAfterMinutes === v ? 'selected' : ''}>${l}</option>`).join('')}</select></div>
        <p class="note">${memoryNote(p.freshAfterMinutes)}</p></div>
    </section>
    <section class="group">
      <div class="row"><div><strong>Global shortcut</strong><p>Show or hide Kreide from anywhere: <kbd>${mac ? '⌘ ⇧ Space' : 'Ctrl Shift Space'}</kbd></p></div>${toggle('shortcutEnabled', 'Global shortcut')}</div>
      <div class="row"><div><strong>Always on top</strong><p>Keep the window above other apps.</p></div>${toggle('alwaysOnTop', 'Always on top')}</div>
    </section>
    <section class="group">
      ${providers.map(v => `<div class="row"><div><strong>${names[v]} sign-in</strong><p>Sign out and clear this app’s ${names[v]} website data.</p></div>
        <button class="button ${confirmReset === v ? 'danger' : ''}" data-reset="${v}">${confirmReset === v ? 'Click to confirm' : 'Sign out'}</button></div>`).join('')}
    </section>
    <p class="footnote">Kreide keeps no chat history. Chats use ${names.chatgpt} Temporary Chat and ${names.claude} Incognito, under those services’ own retention rules.</p>
  </main>`;
  document.querySelectorAll<HTMLButtonElement>('[data-default]').forEach(el => el.addEventListener('click', () => { void save({ defaultProvider: el.dataset.default as Provider }); }));
  document.querySelector<HTMLSelectElement>('#fresh')!.addEventListener('change', e => { void save({ freshAfterMinutes: Number((e.target as HTMLSelectElement).value) }); });
  document.querySelectorAll<HTMLButtonElement>('[data-pref]').forEach(el => el.addEventListener('click', () => {
    const key = el.dataset.pref as 'alwaysOnTop' | 'shortcutEnabled';
    void save({ [key]: !state.preferences[key] });
  }));
  document.querySelectorAll<HTMLButtonElement>('[data-reset]').forEach(el => el.addEventListener('click', async () => {
    const provider = el.dataset.reset as Provider;
    if (confirmReset !== provider) { confirmReset = provider; renderSettings(); return; }
    confirmReset = null;
    const next = await call<Snapshot>('reset_provider', { provider });
    if (next) { state = next; toast(`Signed out of ${names[provider]}.`); }
    renderSettings();
  }));
}
async function save(patch: Partial<Preferences>) {
  const preferences = { ...state.preferences, ...patch };
  if (!desktop) { state.preferences = preferences; renderSettings(); return; }
  const next = await call<Snapshot>('save_preferences', { preferences });
  if (next) state = next;
  renderSettings();
}
async function startSettings() {
  document.body.classList.add('settings-window');
  renderSettings();
  if (!desktop) return;
  await listen<Snapshot>('session-changed', event => { state = event.payload; renderSettings(); });
  const next = await call<Snapshot>('get_state');
  if (next) { state = next; renderSettings(); }
  window.addEventListener('keydown', e => { if (e.key === 'Escape') window.close(); });
}

function escape(text: string) {
  return text.replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
}

void (location.hash === '#settings' ? startSettings() : startMain());
