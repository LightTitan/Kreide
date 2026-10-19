# Kreide

A small Tauri 2 desktop app for one-off questions on **ChatGPT** and **Claude**, using your existing website accounts and subscriptions. No API key or CLI provider is required.

## Run

Prerequisites: Node.js 22.12+ (or 24+), npm, a current stable Rust toolchain, and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). macOS **14+** is required for isolated persistent WKWebView profiles. Windows uses WebView2; Linux needs WebKitGTK 4.1 and tray support.

```sh
cd kreide
npm ci
npm run desktop
```

Build an application bundle:

```sh
npm run desktop:build
```

On macOS the app is generated at `src-tauri/target/release/bundle/macos/Kreide.app`. It is an unsigned local build. Windows and Linux bundles must be built on their respective platforms. No signing or distribution credentials are configured.

`npm run dev` opens a **UI preview only**, at `http://127.0.0.1:1420`. Website views and desktop features require `npm run desktop`.

## Use

- Choose **ChatGPT** or **Claude** and sign in inside the app. Existing browser cookies are not imported.
- ChatGPT opens with a best-effort temporary-chat URL hint. Claude opens a new chat. Kreide attempts only explicitly labeled enable controls; otherwise enable **Temporary** / the **Incognito ghost icon** yourself.
- A recognized active website control/header changes the toolbar to **Temporary mode detected**. Until then, the send guard blocks known composer input and submission paths. A URL flag alone never unlocks sending.
- **End chat** destroys the provider webview. No conversation URL is saved or restored. Your website login profile remains.
- Switching providers asks to end the previous chat first. Only one provider page stays loaded.
- Closing the main window hides it and preserves the active conversation. Use **End chat** to unload it or **Quit Kreide** in the tray to exit.
- **⌘⇧Space** on macOS / **Ctrl+Shift+Space** on Windows and Linux shows or hides the window. The shortcut can be disabled in Settings. Shortcut conflicts are reported without preventing startup.
- Settings control the default provider and Always on top. The small shortcut button on the home toolbar opens your default provider.
- **Reset website data** clears the active provider's isolated local profile and ends the chat. This removes local authentication; it does not revoke server-side sessions or delete provider-held data.

## Privacy and integration limits

The trusted local interface has the app's commands; remote provider and sign-in pages have no capability grants, and every custom Rust command also checks its caller's webview label. Navigation is limited to HTTPS provider and common authentication domains. Popups are attached to the same provider profile and closed when the chat ends. Unknown external navigation and downloads are not part of this version.

Preferences contain only the default provider, Always on top, and shortcut setting. Kreide creates no transcript database, prompt logs, response logs, or conversation restore list. It does retain app-owned browser storage for convenient sign-in. Cookies, caches, service workers, or website storage may contain local conversation remnants; destroying a view is **not** a secure erase.

The send guard is a **best-effort English website UI adapter**, not a provider API or a network firewall. It rechecks visible controls before each known send path. Unrecognized layouts remain locked; there is no manual override. The mode check does not establish personalization settings or provider retention. Unsupported website layouts, new send mechanisms, embedded sign-in restrictions, SSO, CAPTCHAs, and website changes can prevent normal use or defeat a UI-only guard. Do not treat it as a privacy boundary for sensitive information.

The official temporary modes have their own retention rules: [ChatGPT Temporary Chat](https://help.openai.com/en/articles/8914046-temporary-chat-in-chatgpt) and [Claude Incognito Chat](https://support.claude.com/en/articles/12260368-use-incognito-chats). Browser-private mode is a separate feature and is not used as a substitute.

## Verify

```sh
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

The JavaScript tests exercise locked input, Enter/click/paste/drop submission, mode changes, login forms, false-positive labels, and automatic enable behavior. Rust tests exercise navigation boundaries, profile separation, and the preferences schema. They use fixtures, not live authenticated websites.

Before relying on a new provider layout, verify it using an account: sign in, enable the site's real mode, check the toolbar, send a disposable question, change modes and verify sending relocks, end and reopen, then restart and reset the provider. Repeat on each target OS. Never infer successful sign-in or live temporary-mode integration solely from passing fixture tests.

## Source layout

- `src/`: small TypeScript/Vite local UI; no large frontend framework.
- `src-tauri/src/lib.rs`: session lifecycle, profiles, tray, shortcut and native commands.
- `src-tauri/src/provider-focus.js`: document-start provider adapter.
- `tests/`: provider adapter regression tests.

Multiwebview support currently uses Tauri's `unstable` feature. Dependencies are locked in `package-lock.json` and `src-tauri/Cargo.lock`.

## License

MIT. See [LICENSE](LICENSE).
