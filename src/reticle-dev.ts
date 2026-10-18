// Dev-only. Loaded for you after connect() — by @reticlehq/vite-plugin's injected connect, or by
// the connect file init wrote for a framework that connects itself — so you do not import it.
// Self-guards on import.meta.env.DEV, so it is a no-op in a production build.
import { registerCapabilities } from '@reticlehq/browser';

if (import.meta.env.DEV) {
  // ── Start with ONE flow. ─────────────────────────────────────────────────────────────────────
  // You do not need to describe the whole app to get value, and trying to is the slow path. Register
  // the store your most important flow reads, and list the testids that flow touches. Add more later,
  // when a flow you actually replay needs them.
  //
  // Registering a store is the highest-value line in this file: it lets the agent check what the app
  // BELIEVES, not just what it rendered — the class of bug a screenshot cannot see. Pass the STORE,
  // not `() => store.getState()`: the store form wires `subscribe` too, so every mutation emits a
  // state diff; the getter form is read-only and silently produces empty diffs.
  // No state library detected. If you add one, register it here — see https://docs.reticle.sh/state-management

  registerCapabilities({
    testids: ['conversation-panel', 'hosts-view', 'pane-terminal', 'sidebar-expand', 'hosts-overlay', 'assets-overlay', 'accounts-overlay', 'control-overlay', 'automation-overlay', 'dashboard-overlay', 'board-view', 'details-view', 'settings-view', 'inspector', 'health-error', 'bootstrap-error', 'bootstrap-retry', 'footer-version', 'footer-downloads', 'footer-trackers', 'footer-hub-unavailable', 'hub-badge', 'hub-warning', 'footer-shortcuts', 'pane-sidebar', 'resizer-sidebar', '{testid}-input', 'accounts-page', 'accounts-count', 'accounts-refresh-all', 'accounts-all-hosts', 'accounts-add', 'accounts-empty', 'account-card', 'account-counts', 'account-paused', 'account-paused-show', 'account-paused-wait', 'account-paused-switch', 'account-detail', 'account-refresh', 'account-checked', 'account-spend', 'account-window-{kind}', 'account-reset-{kind}', 'account-history-{kind}', 'account-history-empty-{kind}', 'account-logins', 'account-login-host', 'account-paused-all', 'account-sessions', 'account-paused-routines', 'account-missing', 'facet-{f.id}', '{testid}-status', 'add-account-signed-in', 'add-account-open-link', 'add-account-copy-link', 'add-account-device-code', 'add-account-copy-code'],
    signals: [], // names you pass to reticle.signal()
    stores: [], // the keys you registered above
  });
}

// Supply the dev flag type when the host does not include vite/client.
// These interfaces merge with vite/client when it is already present.
declare global {
  interface ImportMetaEnv {
    DEV: boolean;
  }
  interface ImportMeta {
    readonly env: ImportMetaEnv;
  }
}
