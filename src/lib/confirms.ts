import { derived, get, writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { mcpConfirm, mcpPendingConfirms, MCP_CONFIRM_EVENT, type ConfirmRequest } from './mcp';
import { push, pushError } from './toasts';
import { CONFIRM_CHANGED_EVENT } from './events';

/**
 * The queue of control-API calls waiting for a person (redesign step 9.2).
 * One queue, two places to answer it: the operator's own requests — its
 * starts and kills, always confirmed (M9.7) — become cards in Control's
 * transcript while that transcript is on screen
 * (`ConfirmCards`); everything else, and the operator's requests whenever no
 * transcript is showing, stays in the dialog (`McpConfirmDialog`), so a
 * request is never parked where nobody can see it.
 */
export const confirmQueue = writable<ConfirmRequest[]>([]);

/** How many confirm-card hosts (an agent transcript) are mounted now. */
export const cardHosts = writable(0);

/** Requests that belong on a card while a card host is mounted. */
export const cardConfirms = derived([confirmQueue, cardHosts], ([q, hosts]) =>
  hosts > 0 ? q.filter((r) => r.operator) : [],
);

/** Requests the dialog answers: everything the cards do not. */
export const dialogConfirms = derived([confirmQueue, cardHosts], ([q, hosts]) =>
  hosts > 0 ? q.filter((r) => !r.operator) : q,
);

/** Nonces with an answer in flight, so a second click cannot send another. */
export const answering = writable<ReadonlySet<string>>(new Set());

export function enqueueConfirm(req: ConfirmRequest): void {
  confirmQueue.update((q) => (q.some((x) => x.nonce === req.nonce) ? q : [...q, req]));
}

/** Register a confirm-card host for as long as it is mounted. */
export function hostConfirmCards(): () => void {
  cardHosts.update((n) => n + 1);
  return () => cardHosts.update((n) => Math.max(0, n - 1));
}

/**
 * Answer one request. A verdict that never reached the backend must not
 * look like one that did: the request stays queued and the failure is said.
 * Silently dropping a failed DENY would leave the person believing they
 * refused the agent.
 */
export async function answerConfirm(nonce: string, approved: boolean): Promise<boolean> {
  const req = get(confirmQueue).find((r) => r.nonce === nonce);
  if (!req || get(answering).has(nonce)) return false;
  answering.update((s) => new Set([...s, nonce]));
  const r = await mcpConfirm(nonce, approved);
  answering.update((s) => {
    const next = new Set(s);
    next.delete(nonce);
    return next;
  });
  if (!r.ok) {
    pushError(r.error, `${approved ? 'Approving' : 'Denying'} ${req.tool} failed`);
    return false;
  }
  confirmQueue.update((q) => q.filter((x) => x.nonce !== nonce));
  if (r.value === false) {
    // The backend refused the answer: the request expired, or another
    // device answered it first (the first answer wins). Saying nothing here
    // let the person believe their approve or deny was the one that counted.
    push({
      kind: 'error',
      message: `That ${req.tool} request had already been answered or had expired — your ${approved ? 'approval' : 'denial'} was not recorded.`,
    });
    void resync();
    return false;
  }
  return true;
}

let started = false;

/** How often a non-empty queue is re-read so expired requests leave it. */
export const EXPIRY_RESYNC_MS = 30_000;

/**
 * Replace the queue with what the backend lists now. A hub-backed desktop
 * hears of its hub's queue only as `confirm:changed`, and a request answered
 * on another device (the phone) must leave this one too.
 */
async function resync(): Promise<void> {
  const r = await mcpPendingConfirms();
  if (!r.ok || !Array.isArray(r.value)) return;
  confirmQueue.set(r.value.map(normalise));
}

/**
 * Seed the queue with requests raised before the window mounted (a reload)
 * and follow new ones: `mcp:confirm-required` from this desktop's own
 * server, `confirm:changed` from a hub. Call once, from the always-mounted
 * dialog.
 */
export function startConfirmQueue(): () => void {
  if (started) return () => {};
  started = true;
  const unlisteners: UnlistenFn[] = [];
  let disposed = false;
  const keep = (fn: UnlistenFn) => {
    if (disposed) fn();
    else unlisteners.push(fn);
  };
  void mcpPendingConfirms().then((r) => {
    if (r.ok && Array.isArray(r.value)) for (const p of r.value) enqueueConfirm(normalise(p));
  });
  void listen<ConfirmRequest>(MCP_CONFIRM_EVENT, (e) => enqueueConfirm(normalise(e.payload))).then(keep);
  void listen(CONFIRM_CHANGED_EVENT, () => void resync()).then(keep);
  // A request that expires announces nothing, so re-read the queue now and
  // then while something is on it: otherwise its card or dialog stays up
  // until an unrelated change, and answering it only reports "expired".
  const expiry = setInterval(() => {
    if (get(confirmQueue).length > 0) void resync();
  }, EXPIRY_RESYNC_MS);
  keep(() => clearInterval(expiry));
  return () => {
    disposed = true;
    started = false;
    for (const u of unlisteners.splice(0)) u();
  };
}

/** An older backend lists only nonce and tool; fill in the rest. */
function normalise(p: Partial<ConfirmRequest> & { nonce: string; tool: string }): ConfirmRequest {
  return {
    nonce: p.nonce,
    tool: p.tool,
    summary: p.summary ?? '',
    caller: p.caller ?? '',
    operator: p.operator ?? false,
    asked_at: p.asked_at ?? 0,
  };
}

/** Test seam: empty the queue and the in-flight set. */
export function resetConfirmsForTests(): void {
  confirmQueue.set([]);
  answering.set(new Set());
  cardHosts.set(0);
}
