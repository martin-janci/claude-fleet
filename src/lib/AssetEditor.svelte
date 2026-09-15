<script lang="ts">
  import { untrack } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import {
    updateAsset, addResource, removeResource, lintAsset, getAsset, resourceSize,
    TOOLS, TIERS, EVENTS, KIND_FIELDS,
    type EditableAsset, type LintReport, type WriteResult,
  } from './assets';
  import ConfirmDialog from './ConfirmDialog.svelte';

  // Header/body/kind-field edits are staged locally and only committed by
  // Save (`updateAsset`). Resource add/remove are separate backend
  // operations (`catalog_add_resource`/`catalog_remove_resource`) that
  // commit immediately on their own — they never wait for Save, and after
  // each one this component re-fetches the asset (`getAsset`) to pick up
  // the fresh resource list, syncing it into both `draft` and the
  // `initial` baseline so an unrelated header edit's "changed" state is
  // never thrown off by a resource op that already landed.
  let {
    asset,
    onsaved,
    oncancel,
  }: {
    asset: EditableAsset;
    onsaved: (result: WriteResult) => void;
    oncancel: () => void;
  } = $props();

  function clone(a: EditableAsset): EditableAsset {
    return JSON.parse(JSON.stringify(a));
  }

  let initial = $state(untrack(() => clone(asset)));
  let draft = $state(untrack(() => clone(asset)));

  let saving = $state(false);
  let saveError = $state<string | null>(null);
  // The last known server-side lint: seeded from `catalog_lint_asset`
  // against the stored asset on mount, replaced by the report an `E_LINT`
  // save failure carries (the more relevant one once that happens). Never
  // consulted for Save's disabled state — only the client checks are, since
  // this is always a report on some *other* version of the asset (the
  // stored one, or the draft as of the last failed attempt), not the
  // current draft.
  let serverLint = $state<LintReport | null>(null);
  let resourceBusy = $state<string | null>(null);
  let resourceError = $state<string | null>(null);

  $effect(() => {
    const k = draft.kind, n = draft.name;
    lintAsset(k, n).then((r) => {
      if (r.ok) serverLint = r.value;
    });
  });

  // Compares everything except the resources' base64 `bytes` payloads: those
  // can be multi-MB, and `changed` is a `$derived` that re-stringifies on
  // every reactive update (including each keystroke in the body textarea).
  // A projection of `[rel_path, bytes.length]` still catches an add/remove/
  // replace (refreshResources() swaps the whole array in on a resource op)
  // without paying to serialize the payload itself.
  function nonResourceProjection(a: EditableAsset): Omit<EditableAsset, 'resources'> {
    const { resources: _resources, ...rest } = a;
    return rest;
  }
  function resourceProjection(a: EditableAsset): [string, number][] {
    return (a.resources ?? []).map((r) => [r.rel_path, r.bytes.length]);
  }
  const changed = $derived(
    JSON.stringify(nonResourceProjection(draft)) !== JSON.stringify(nonResourceProjection(initial)) ||
    JSON.stringify(resourceProjection(draft)) !== JSON.stringify(resourceProjection(initial)),
  );

  const NAME_RE = /^[a-z0-9][a-z0-9-]*$/;
  // Kept deliberately minimal per spec: required description and the name
  // pattern. Everything else (vocab, TODOs, per-kind requirements) is the
  // server lint's job — shown as `serverLint`/the E_LINT report, not
  // duplicated here.
  const clientErrors = $derived.by((): string[] => {
    const errs: string[] = [];
    if (draft.description.trim() === '') errs.push('description must not be empty');
    if (!NAME_RE.test(draft.name)) errs.push("name must match [a-z0-9][a-z0-9-]*");
    return errs;
  });

  const canSave = $derived(clientErrors.length === 0 && changed && !saving);

  // ── Header fields ────────────────────────────────────────────────────
  const tagsText = $derived((draft.tags ?? []).join(', '));
  function onTagsInput(e: Event) {
    const v = (e.currentTarget as HTMLInputElement).value;
    draft.tags = v.split(',').map((t) => t.trim()).filter((t) => t !== '');
  }

  // ── Kind-specific fields ─────────────────────────────────────────────
  const fields = $derived(KIND_FIELDS[draft.kind] ?? []);

  function strArr(field: string): string[] {
    return (draft[field] as string[] | undefined) ?? [];
  }
  function boolField(field: string): boolean {
    return Boolean(draft[field]);
  }
  function setBoolField(field: string, value: boolean) {
    draft[field] = value;
  }
  function strField(field: string): string {
    return (draft[field] as string | undefined) ?? '';
  }
  function setStrField(field: string, value: string) {
    draft[field] = value;
  }
  function setStrArr(field: string, arr: string[]) {
    draft[field] = arr;
  }
  function toggleTool(field: string, tool: string) {
    const arr = strArr(field);
    setStrArr(field, arr.includes(tool) ? arr.filter((t) => t !== tool) : [...arr, tool]);
  }
  function listText(field: string): string {
    return strArr(field).join(', ');
  }
  function onListInput(field: string, e: Event) {
    const v = (e.currentTarget as HTMLInputElement).value;
    setStrArr(field, v.split(',').map((t) => t.trim()).filter((t) => t !== ''));
  }

  function actionField(name: 'type' | 'command' | 'url'): string {
    const action = (draft.action as Record<string, unknown> | undefined) ?? {};
    return (action[name] as string | undefined) ?? (name === 'type' ? 'command' : '');
  }
  function setActionField(name: 'type' | 'command' | 'url', value: string) {
    const action = { ...((draft.action as Record<string, unknown> | undefined) ?? {}) };
    // Switching `type` away from a transport drops the sibling field that
    // belongs only to the other transport (`command` for `http`, `url` for
    // `command`) — otherwise the stale key rides along into the saved YAML
    // and both get rendered into the host's hook config.
    if (name === 'type') {
      if (value === 'command') delete action.url;
      else if (value === 'http') delete action.command;
    }
    action[name] = value;
    draft.action = action;
  }

  function marketplaceField(name: 'name' | 'source' | 'repo'): string {
    const m = (draft.marketplace as Record<string, unknown> | undefined) ?? {};
    return (m[name] as string | undefined) ?? '';
  }
  function setMarketplaceField(name: 'name' | 'source' | 'repo', value: string) {
    const m = { ...((draft.marketplace as Record<string, unknown> | undefined) ?? {}) };
    m[name] = value;
    draft.marketplace = m;
  }

  function envText(): string {
    const env = (draft.env as Record<string, string> | undefined) ?? {};
    return Object.entries(env).map(([k, v]) => `${k}=${v}`).join('\n');
  }
  function onEnvInput(e: Event) {
    const v = (e.currentTarget as HTMLTextAreaElement).value;
    const env: Record<string, string> = {};
    for (const line of v.split('\n')) {
      const t = line.trim();
      if (t === '') continue;
      const eq = t.indexOf('=');
      if (eq === -1) continue;
      env[t.slice(0, eq).trim()] = t.slice(eq + 1).trim();
    }
    draft.env = env;
  }

  // ── Resources ────────────────────────────────────────────────────────
  const resources = $derived(draft.resources ?? []);

  async function refreshResources() {
    const r = await getAsset(draft.kind, draft.name);
    if (!r.ok) return;
    const fresh = r.value.asset.resources ?? [];
    // `draft` and `initial` must not alias the same array/objects — nothing
    // mutates a resource in place today, but assigning the same reference to
    // both is a trap for the next person who does.
    draft.resources = fresh.map((res) => ({ ...res }));
    initial.resources = fresh.map((res) => ({ ...res }));
  }

  async function addResourceFile() {
    resourceError = null;
    let picked: unknown;
    try {
      picked = await open({ multiple: false });
    } catch (e) {
      resourceError = e instanceof Error ? e.message : String(e);
      return;
    }
    if (typeof picked !== 'string') return;
    resourceBusy = picked;
    const r = await addResource(draft.kind, draft.name, picked);
    resourceBusy = null;
    if (!r.ok) {
      resourceError = r.error.message;
      return;
    }
    await refreshResources();
  }

  // Resource removal commits immediately on the backend (see the note atop
  // the component), independent of Save/Cancel — so a mis-click is not
  // reversible the way an unsaved header edit is. Gate it behind
  // ConfirmDialog: the Remove button only stages `resourceToRemove`; the
  // backend call happens in `confirmRemoveResource`.
  let resourceToRemove = $state<string | null>(null);

  function requestRemoveResource(relPath: string) {
    resourceToRemove = relPath;
  }

  async function removeResourceRow(relPath: string) {
    resourceError = null;
    resourceBusy = relPath;
    const r = await removeResource(draft.kind, draft.name, relPath);
    resourceBusy = null;
    if (!r.ok) {
      resourceError = r.error.message;
      return;
    }
    await refreshResources();
  }

  async function confirmRemoveResource() {
    if (resourceToRemove === null) return;
    const relPath = resourceToRemove;
    await removeResourceRow(relPath);
    resourceToRemove = null;
  }

  // ── Save / Cancel ────────────────────────────────────────────────────
  async function save() {
    if (!canSave) return;
    saving = true;
    saveError = null;
    const r = await updateAsset(draft);
    saving = false;
    if (!r.ok) {
      if (r.error.code === 'E_LINT' && r.error.details) {
        serverLint = r.error.details as LintReport;
      }
      saveError = r.error.message;
      return;
    }
    onsaved(r.value);
  }
</script>

<div class="editor">
  <div class="row">
    <label>Name <input value={draft.name} readonly data-testid="editor-name" /></label>
    <label>Version <input bind:value={draft.version} data-testid="editor-version" /></label>
  </div>
  <label>Description
    <textarea bind:value={draft.description} rows="2" data-testid="editor-description"></textarea>
  </label>
  <label>Tags (comma separated)
    <input value={tagsText} oninput={onTagsInput} data-testid="editor-tags" />
  </label>

  <div class="kind-fields">
    {#if draft.kind === 'skill'}
      {#if fields.includes('allowed_tools')}
        <div class="field" data-testid="editor-field-allowed_tools">
          <span class="field-label">Allowed tools</span>
          <div class="tool-list">
            {#each TOOLS as t (t)}
              <label class="tool">
                <input
                  type="checkbox"
                  checked={strArr('allowed_tools').includes(t)}
                  onchange={() => toggleTool('allowed_tools', t)}
                  data-testid={`editor-tool-allowed_tools-${t}`}
                />{t}
              </label>
            {/each}
          </div>
        </div>
      {/if}
      {#if fields.includes('user_invocable')}
        <label class="field" data-testid="editor-field-user_invocable">
          <input
            type="checkbox"
            checked={boolField('user_invocable')}
            onchange={(e) => setBoolField('user_invocable', (e.currentTarget as HTMLInputElement).checked)}
          /> User invocable
        </label>
      {/if}
      {#if fields.includes('triggers')}
        <label class="field" data-testid="editor-field-triggers">
          Triggers (comma separated)
          <input value={listText('triggers')} oninput={(e) => onListInput('triggers', e)} />
        </label>
      {/if}
    {:else if draft.kind === 'agent'}
      {#if fields.includes('tools')}
        <div class="field" data-testid="editor-field-tools">
          <span class="field-label">Tools</span>
          <div class="tool-list">
            {#each TOOLS as t (t)}
              <label class="tool">
                <input
                  type="checkbox"
                  checked={strArr('tools').includes(t)}
                  onchange={() => toggleTool('tools', t)}
                  data-testid={`editor-tool-tools-${t}`}
                />{t}
              </label>
            {/each}
          </div>
        </div>
      {/if}
      {#if fields.includes('model')}
        <label class="field" data-testid="editor-field-model">
          Model
          <select value={strField('model')} onchange={(e) => setStrField('model', (e.currentTarget as HTMLSelectElement).value)}>
            {#each TIERS as t (t)}<option value={t}>{t}</option>{/each}
          </select>
        </label>
      {/if}
    {:else if draft.kind === 'hook'}
      {#if fields.includes('event')}
        <label class="field" data-testid="editor-field-event">
          Event
          <select value={strField('event')} onchange={(e) => setStrField('event', (e.currentTarget as HTMLSelectElement).value)}>
            {#each EVENTS as e (e)}<option value={e}>{e}</option>{/each}
          </select>
        </label>
      {/if}
      {#if fields.includes('action')}
        <div class="field" data-testid="editor-field-action">
          <span class="field-label">Action</span>
          <label>
            Type
            <select value={actionField('type')} onchange={(e) => setActionField('type', (e.currentTarget as HTMLSelectElement).value)} data-testid="editor-field-action-type">
              <option value="command">command</option>
              <option value="http">http</option>
            </select>
          </label>
          {#if actionField('type') === 'command'}
            <label>
              Command
              <input value={actionField('command')} oninput={(e) => setActionField('command', (e.currentTarget as HTMLInputElement).value)} data-testid="editor-field-action-command" />
            </label>
          {:else}
            <label>
              URL
              <input value={actionField('url')} oninput={(e) => setActionField('url', (e.currentTarget as HTMLInputElement).value)} data-testid="editor-field-action-url" />
            </label>
          {/if}
        </div>
      {/if}
    {:else if draft.kind === 'mcp_server'}
      {#if fields.includes('transport')}
        <label class="field" data-testid="editor-field-transport">
          Transport
          <select value={strField('transport')} onchange={(e) => setStrField('transport', (e.currentTarget as HTMLSelectElement).value)}>
            <option value="http">http</option>
            <option value="stdio">stdio</option>
          </select>
        </label>
      {/if}
      {#if fields.includes('url')}
        <label class="field" data-testid="editor-field-url">
          URL
          <input value={strField('url')} oninput={(e) => setStrField('url', (e.currentTarget as HTMLInputElement).value)} />
        </label>
      {/if}
      {#if fields.includes('command')}
        <label class="field" data-testid="editor-field-command">
          Command
          <input value={strField('command')} oninput={(e) => setStrField('command', (e.currentTarget as HTMLInputElement).value)} />
        </label>
      {/if}
      {#if fields.includes('args')}
        <label class="field" data-testid="editor-field-args">
          Args (comma separated)
          <input value={listText('args')} oninput={(e) => onListInput('args', e)} />
        </label>
      {/if}
      {#if fields.includes('env')}
        <label class="field" data-testid="editor-field-env">
          Env (KEY=VALUE per line)
          <textarea value={envText()} oninput={onEnvInput} rows="3"></textarea>
        </label>
      {/if}
    {:else if draft.kind === 'plugin_ref'}
      {#if fields.includes('harness')}
        <label class="field" data-testid="editor-field-harness">
          Harness
          <input value={strField('harness')} oninput={(e) => setStrField('harness', (e.currentTarget as HTMLInputElement).value)} />
        </label>
      {/if}
      {#if fields.includes('marketplace')}
        <div class="field" data-testid="editor-field-marketplace">
          <span class="field-label">Marketplace</span>
          <label>Name <input value={marketplaceField('name')} oninput={(e) => setMarketplaceField('name', (e.currentTarget as HTMLInputElement).value)} data-testid="editor-field-marketplace-name" /></label>
          <label>Source <input value={marketplaceField('source')} oninput={(e) => setMarketplaceField('source', (e.currentTarget as HTMLInputElement).value)} data-testid="editor-field-marketplace-source" /></label>
          <label>Repo <input value={marketplaceField('repo')} oninput={(e) => setMarketplaceField('repo', (e.currentTarget as HTMLInputElement).value)} data-testid="editor-field-marketplace-repo" /></label>
        </div>
      {/if}
      {#if fields.includes('plugin')}
        <label class="field" data-testid="editor-field-plugin">
          Plugin
          <input value={strField('plugin')} oninput={(e) => setStrField('plugin', (e.currentTarget as HTMLInputElement).value)} />
        </label>
      {/if}
    {/if}
  </div>

  <label>Body
    <textarea bind:value={draft.body} rows="10" class="body" data-testid="editor-body"></textarea>
  </label>

  <div class="resources">
    <h4>Resources</h4>
    {#if resourceError}<p class="error" data-testid="editor-resource-error">{resourceError}</p>{/if}
    {#each resources as r (r.rel_path)}
      <div class="resource-row" data-testid={`editor-resource-${r.rel_path}`}>
        <span class="rel">{r.rel_path}</span>
        <span class="size">{resourceSize(r.bytes)} bytes</span>
        <button
          type="button"
          onclick={() => requestRemoveResource(r.rel_path)}
          disabled={resourceBusy === r.rel_path}
          data-testid={`editor-resource-remove-${r.rel_path}`}
        >Remove</button>
      </div>
    {/each}
    {#if resources.length === 0}<p class="muted">No resources.</p>{/if}
    <button type="button" onclick={addResourceFile} disabled={resourceBusy !== null} data-testid="editor-resource-add">Add file…</button>
  </div>

  {#if serverLint && (serverLint.errors.length > 0 || serverLint.warnings.length > 0)}
    <div class="lint" data-testid="editor-lint">
      {#each serverLint.errors as f, i (i)}<p class="lint-error" data-testid={`editor-lint-error-${i}`}>{f.field}: {f.message}</p>{/each}
      {#each serverLint.warnings as f, i (i)}<p class="lint-warn" data-testid={`editor-lint-warning-${i}`}>{f.field}: {f.message}</p>{/each}
    </div>
  {/if}
  {#if clientErrors.length > 0}
    <div class="client-errors" data-testid="editor-client-errors">
      {#each clientErrors as e, i (i)}<p class="lint-error" data-testid={`editor-client-error-${i}`}>{e}</p>{/each}
    </div>
  {/if}
  {#if saveError}<p class="error" data-testid="editor-save-error">{saveError}</p>{/if}

  <div class="actions">
    <button type="button" onclick={oncancel} disabled={saving} data-testid="editor-cancel">Cancel</button>
    <button type="button" class="primary" onclick={save} disabled={!canSave} data-testid="editor-save">{saving ? 'Saving…' : 'Save'}</button>
  </div>
</div>

{#if resourceToRemove !== null}
  <ConfirmDialog
    title="Remove resource?"
    confirmLabel="Remove"
    danger
    busy={resourceBusy === resourceToRemove}
    onconfirm={confirmRemoveResource}
    oncancel={() => (resourceToRemove = null)}
    confirmTestId="editor-resource-remove-confirm"
  >
    This removes <code>{resourceToRemove}</code> from the asset and commits the removal immediately, independent of Save.
  </ConfirmDialog>
{/if}

<style>
  .editor { display: flex; flex-direction: column; gap: 10px; font-size: 13px; }
  .row { display: flex; gap: 10px; }
  .row label { flex: 1; }
  label { display: flex; flex-direction: column; gap: 3px; font-size: 12px; color: var(--fg-muted); }
  input, select, textarea { font: inherit; padding: 4px 6px; border: 1px solid var(--border); background: var(--bg-pane); color: var(--fg); border-radius: 4px; }
  textarea.body { font-family: ui-monospace, monospace; }
  .kind-fields { display: flex; flex-direction: column; gap: 8px; border: 1px solid var(--border); border-radius: 6px; padding: 8px; }
  .field { display: flex; flex-direction: column; gap: 4px; }
  .field-label { font-size: 11px; text-transform: uppercase; color: var(--fg-muted); }
  .tool-list { display: flex; flex-wrap: wrap; gap: 6px; }
  .tool { flex-direction: row; align-items: center; gap: 4px; font-size: 12px; }
  .resources { border-top: 1px solid var(--border); padding-top: 8px; }
  .resources h4 { margin: 0 0 6px; font-size: 11px; text-transform: uppercase; color: var(--fg-muted); }
  .resource-row { display: flex; align-items: center; gap: 8px; font-size: 12px; padding: 2px 0; }
  .resource-row .rel { font-family: ui-monospace, monospace; }
  .resource-row .size { color: var(--fg-muted); }
  .lint { display: flex; flex-direction: column; gap: 2px; }
  .lint-error { color: #dc2626; margin: 0; font-size: 12px; }
  .lint-warn { color: #d97706; margin: 0; font-size: 12px; }
  .error { color: #dc2626; margin: 0; }
  .muted { color: var(--fg-muted); font-size: 12px; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; }
  .actions button { font-size: 0.85rem; padding: 0.3rem 0.8rem; border: 1px solid var(--border); background: transparent; color: var(--fg); border-radius: 4px; cursor: pointer; }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary { border-color: var(--accent); }
</style>
