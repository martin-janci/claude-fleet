// Fake Tauri IPC for the federation e2e harness (docs/federation-e2e.md).
//
// Injected into the page BEFORE the app loads (Playwright `addInitScript`),
// after `window.__FLEET_WORLD__` has been set to a scenario's world. It
// stands in for the desktop backend: `window.__TAURI_INTERNALS__.invoke`
// answers every command the frontend sends, and the event plugin delivers
// `listen` callbacks, so the real Svelte app runs unchanged in Chromium.
//
// The world is a set of fake hubs: `primary` (the hub this window resolved
// to at launch, or this machine when standalone) and any number of extra
// fleets. A command routes the way `backend/fleets.rs` routes it:
//   - no `fleet` argument, or `fleet: "primary"`  -> the primary hub;
//   - `fleet: "<id>"` on a FLEET_SCOPED command    -> that extra hub;
//   - an extra fleet that is offline / unavailable -> E_HUB_UNREACHABLE.
// Plain JavaScript on purpose: it is injected as a string, never bundled.
//
// `window.__fleetFake` is the test's handle: `calls` (every invoke, in
// order), `unknown` (commands nothing answered), `emit(event, payload)`,
// `setFleet(id, patch)` and `setHub(id, patch)` to change the world while
// the page runs.
(() => {
  'use strict';
  const world = window.__FLEET_WORLD__ || { primary: { hosts: [], sessions: [] }, extras: [] };
  const NOW = Math.floor(Date.now() / 1000);

  // ── row builders (the shapes of src/lib/hosts_fixture.ts) ──────────────
  function host(alias, over) {
    return {
      alias,
      ssh_alias: alias === 'local' ? null : alias,
      reachable: true,
      claude_version: '2.1.145',
      tmux_version: '3.5a',
      hidden: false,
      last_pinged_at: NOW - 30,
      account_uuid: null,
      provisioned: true,
      transport: 'ssh',
      org_id: null,
      claude_version_at: NOW - 600,
      ...over,
    };
  }
  let nextSessionId = 1;
  function session(hostAlias, name, over) {
    return {
      id: nextSessionId++,
      tmux_name: name,
      host_alias: hostAlias,
      project_id: null,
      worktree_id: null,
      created_at: NOW - 3600,
      last_activity_at: NOW - 60,
      status: 'running',
      notes: null,
      account_uuid: null,
      kind: 'work',
      reviews_session_id: null,
      worktree_key: null,
      lost_at: null,
      claude_session_id: null,
      claude_status: 'idle',
      effort_level: null,
      pr_url: null,
      current_activity: null,
      context_pct: null,
      stuck_kind: null,
      friendly_name: null,
      safe_kill_state: null,
      safe_kill_nonce: null,
      safe_kill_detail: null,
      safe_kill_requested_at: null,
      idle_since: NOW - 120,
      stuck_since: null,
      last_playbook_at: null,
      last_prompt: null,
      started_at: NOW - 3600,
      last_turn_at: NOW - 120,
      ci_status: null,
      turn_seq: 1,
      last_stop_at: NOW - 120,
      parent_session_id: null,
      tags: [],
      model: null,
      context_tokens: null,
      context_window: null,
      context_source: null,
      context_at: null,
      context_stale: false,
      tmux_pane_id: null,
      pending_input: null,
      ...over,
    };
  }

  // Each hub numbers its own sessions from 1, as real hubs do: the ids
  // collide across fleets on purpose (fleets.ts keys them apart).
  function buildHub(spec) {
    nextSessionId = 1;
    return {
      label: spec.label,
      url: spec.url ?? null,
      hosts: (spec.hosts || []).map((h) => (typeof h === 'string' ? host(h) : host(h.alias, h))),
      sessions: (spec.sessions || []).map((s) => session(s.host, s.name, s)),
      projects: spec.projects || [],
      orgs: spec.orgs || [],
      accounts: spec.accounts || [],
      profile: spec.profile ?? { person_id: 1, name: 'martin', display_name: 'Martin', email: 'martin@example.test', is_owner: true, sso: [], env: [], git: null },
    };
  }

  const hubs = new Map();
  hubs.set('primary', buildHub({ label: 'This fleet', ...world.primary }));
  const fleetMeta = new Map();
  for (const x of world.extras || []) {
    hubs.set(x.id, buildHub(x));
    fleetMeta.set(x.id, {
      id: x.id,
      label: x.label,
      url: x.url ?? `https://${x.id}.example.test`,
      color: x.color ?? '#c2410c',
      primary: false,
      local: false,
      client_name: x.client_name ?? 'test-desktop',
      connection: x.connection === undefined ? { state: 'connected' } : x.connection,
      unavailable: x.unavailable ?? null,
      fleet_id: x.fleet_id ?? `fleet-${x.id}`,
      duplicate_of: x.duplicate_of ?? null,
    });
  }

  const remotePrimary = !!world.primary?.url;
  function primaryInfo() {
    return {
      id: 'primary',
      label: world.primary?.label ?? (remotePrimary ? 'Own hub' : 'This machine'),
      url: world.primary?.url ?? null,
      color: world.primary?.color ?? '#2563eb',
      primary: true,
      local: !remotePrimary,
      client_name: remotePrimary ? 'test-desktop' : null,
      connection: remotePrimary ? { state: 'connected' } : { state: 'standalone' },
      unavailable: null,
      fleet_id: 'fleet-primary',
      duplicate_of: null,
    };
  }
  const fleetsList = () => [primaryInfo(), ...fleetMeta.values()];

  // ── events ────────────────────────────────────────────────────────────
  const callbacks = new Map();
  const listeners = new Map(); // event -> [callback id]
  let nextCb = 1;
  function transformCallback(cb, once) {
    const id = nextCb++;
    callbacks.set(id, (data) => {
      if (once) callbacks.delete(id);
      return cb && cb(data);
    });
    return id;
  }
  let eventSeq = 1;
  function emit(event, payload) {
    for (const id of listeners.get(event) || []) {
      const cb = callbacks.get(id);
      if (cb) cb({ event, id: eventSeq++, payload });
    }
  }

  // ── commands ──────────────────────────────────────────────────────────
  const unreachable = (id) => {
    const m = fleetMeta.get(id);
    const why = m?.unavailable || m?.connection?.reason || 'the hub does not answer';
    return { code: 'E_HUB_UNREACHABLE', message: `${m?.label ?? id}: ${why}` };
  };
  const isDown = (id) => {
    const m = fleetMeta.get(id);
    if (!m) return false;
    const s = m.connection?.state;
    return !!m.unavailable || s === 'offline' || s === 'reconnecting' || m.connection === null;
  };

  const hubStatus = () => ({
    remote: remotePrimary,
    url: world.primary?.url ?? null,
    client_name: remotePrimary ? 'test-desktop' : null,
    client_mode: remotePrimary ? 'full' : null,
    configured_url: world.primary?.url ?? null,
    configured_client_name: remotePrimary ? 'test-desktop' : null,
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  });

  // Answers that do not depend on which hub is asked.
  const STATIC = {
    health_check: () => ({ version: '0.0.0-e2e', db_ready: true, schema_version: 1 }),
    hub_status: hubStatus,
    hub_connection: () => (remotePrimary ? { state: 'connected' } : { state: 'standalone' }),
    fleets_list: fleetsList,
    'plugin:app|version': () => '0.0.0-e2e',
    'plugin:app|name': () => 'Orbit Fleet',
    'plugin:app|tauri_version': () => '2.0.0',
    check_local_prereqs: () => ({ tmux: true, claude: true, git: true, ssh: true }),
    operator_status: () => ({ ready: false, session: null, blocked: 'no_mcp' }),
    mcp_status: () => ({ enabled: false, running: false, port: 4180, token: 'e2e', url: 'http://127.0.0.1:4180/mcp', bind_error: null }),
    pty_drain: () => ({ data: '', bytes: 0 }),
    repo_tree: () => ({ entries: [], truncated: false }),
    capture_session: () => '',
    // Object answers the catch-all below would get wrong (it answers a
    // `list_*` it does not know with []).
    list_pages: () => ({ pages: [], sources: [], resources: [], actions: [] }),
    list_guides: () => ({ guides: [], proposals: [], can_write: false, approvals: [] }),
    fleet_retry_now: (a) => {
      const id = a?.args?.id;
      const m = fleetMeta.get(id);
      if (m && world.retryBringsBack?.includes(id)) {
        m.connection = { state: 'connected' };
        m.unavailable = null;
      }
      return null;
    },
  };

  // Answers read from the hub the call is routed to.
  const PER_HUB = {
    list_hosts: (h) => h.hosts,
    list_sessions: (h) => h.sessions,
    list_projects: (h) => h.projects,
    list_orgs: (h) => h.orgs,
    list_accounts: (h) => h.accounts,
    list_account_usage: () => [],
    my_grants: () => ({ person_id: 1, grants: [] }),
    my_profile: (h) => h.profile,
    list_pull_requests: () => ({ items: [], truncated: false }),
    list_trackers: () => [],
    list_devices: () => [],
    list_people: () => [],
    list_groups: () => [],
  };

  const calls = [];
  const unknown = new Set();
  const violations = [];
  const SCOPED = Array.isArray(window.__FLEET_SCOPED__) ? new Set(window.__FLEET_SCOPED__) : null;

  async function invoke(cmd, args) {
    calls.push({ cmd, args: args ?? null, at: Date.now() });
    if (cmd.startsWith('plugin:event|')) {
      if (cmd === 'plugin:event|listen') {
        if (!listeners.has(args.event)) listeners.set(args.event, []);
        listeners.get(args.event).push(args.handler);
        return args.handler;
      }
      if (cmd === 'plugin:event|unlisten') {
        const l = listeners.get(args.event) || [];
        const i = l.indexOf(args.eventId);
        if (i >= 0) l.splice(i, 1);
        return null;
      }
      if (cmd === 'plugin:event|emit') {
        emit(args.event, args.payload);
        return null;
      }
      return null;
    }
    if (STATIC[cmd]) return STATIC[cmd](args);
    const fleet = args && typeof args.fleet === 'string' ? args.fleet : 'primary';
    if (fleet !== 'primary' && SCOPED && !SCOPED.has(cmd)) {
      // The backend would ignore the argument and act on the PRIMARY hub:
      // exactly the wrong-hub bug multi-hub design D6 rules out.
      violations.push(`${cmd}→${fleet}`);
    }
    if (fleet !== 'primary') {
      if (!hubs.has(fleet)) throw { code: 'E_INVALID', message: `no fleet ${fleet}` };
      if (isDown(fleet)) throw unreachable(fleet);
    }
    const hub = hubs.get(fleet);
    if (PER_HUB[cmd]) return structuredClone(PER_HUB[cmd](hub, args));
    unknown.add(cmd);
    // Lists answer empty, everything else null: the shape a fresh hub gives.
    if (/^(list_|get_.*s$)/.test(cmd) || /_list$/.test(cmd)) return [];
    return null;
  }

  window.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback,
    unregisterCallback: (id) => callbacks.delete(id),
    runCallback: (id, data) => callbacks.get(id)?.(data),
    callbacks,
    convertFileSrc: (p) => p,
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { windowLabel: 'main', label: 'main' },
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (_event, id) => callbacks.delete(id),
  };

  window.__fleetFake = {
    calls,
    unknown,
    violations,
    emit,
    hubs,
    fleetMeta,
    /** Patch an extra fleet's FleetInfo (connection, unavailable…) and
     *  tell the window, as the backend's `fleet:changed` would. */
    setFleet(id, patch) {
      Object.assign(fleetMeta.get(id), patch);
      emit('fleet:event', { fleet: id, name: 'hub:connection', payload: fleetMeta.get(id).connection });
    },
  };
})();
