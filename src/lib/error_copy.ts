import type { IpcError } from './result';

/**
 * Review round 13 (error states): what a person reads when a backend call
 * fails. Step 1.3's rule is no raw codes and no transport noise in user
 * text: the codes below carry a message written for logs ("ssh: connect to
 * host mercury port 22: Connection timed out", "database is locked",
 * "error sending request … (os error 111)"), so the person gets one plain
 * sentence and the original goes under Details. Every other code already
 * carries a sentence the backend wrote for people and is shown as it is,
 * minus a leading `E_*:` some call sites prepend.
 */
const PLAIN: Record<string, string> = {
  E_SSH: "Couldn't reach the host over SSH",
  E_SSH_TIMEOUT: 'The host took too long to answer over SSH',
  E_HOST_OFFLINE: 'The host is offline',
  E_AGENT_OFFLINE: "The host's agent is offline",
  E_AGENT_PROTOCOL: "The host's agent answered in a way this version doesn't understand",
  E_HUB_UNREACHABLE: "Couldn't reach the hub",
  E_HUB_TIMEOUT: 'The hub took too long to answer',
  E_HUB_PROTOCOL: "The hub answered in a way this version doesn't understand",
  E_TIMEOUT: 'That took too long to answer',
  E_PTY_CLOSED: 'The terminal connection closed',
  E_PTY_BUSY: 'The terminal is busy; try again in a moment',
  E_SQLITE: "The local database couldn't do that",
  E_SERIALIZE: 'Something went wrong reading the answer',
  E_PARSE: 'Something went wrong reading the answer',
  E_INTERNAL: 'Something went wrong inside Orbit Fleet',
  E_RATE_LIMITED: 'Too many requests; try again in a moment',
  // (No `E_HUB_UNAVAILABLE` / `E_UNAUTHORIZED` row: one fixed sentence about
  // this app's own pairing was wrong for an extra fleet and for a standalone
  // update check. `rewordedFor` words them from the message.)
};

/** Review w16-8: these codes mean the request was sent and its outcome is
 *  unknown (`IpcError::may_have_run` in `ipc_error.rs`), so the sentence must
 *  not read as "it failed": a second Kill after a timed-out first one only
 *  finds nothing to kill. Only for a write (`ErrorTextOptions.write`): a read
 *  has nothing to go through, and Retry is its next step. */
const MAY_HAVE_RUN = new Set(['E_SSH_TIMEOUT', 'E_TIMEOUT', 'E_AGENT_PROTOCOL']);
const MAY_HAVE_RUN_TAIL = 'It may have gone through anyway. The list refreshes; check it before trying again';

function mayHaveRun(e: Pick<IpcError, 'code' | 'details'>): boolean {
  if (connectionLost(e)) return false; // CONNECTION_LOST already says so
  if (MAY_HAVE_RUN.has(e.code)) return true;
  return e.code === 'E_HUB_TIMEOUT' && (e.details as { outcome_unknown?: unknown } | null | undefined)?.outcome_unknown === true;
}

/** Review w16-7: an `E_LOCAL_ONLY` refusal opens with the Tauri command name
 *  and the hub's URL (`backend/routing.rs`); the person needs neither. */
const LOCAL_ONLY_PREFIX = /^.+? is not available while this desktop is a window onto \S+?;\s*/;

function localOnlyText(msg: string): string | undefined {
  const m = LOCAL_ONLY_PREFIX.exec(msg);
  if (!m) return undefined;
  const rest = msg.slice(m[0].length).trim();
  return rest
    ? `Not available on a desktop paired with a hub. ${rest.charAt(0).toUpperCase()}${rest.slice(1)}`
    : 'Not available on a desktop paired with a hub';
}

/** Review w16-2: `E_ACCOUNT_LIMIT` from the hub's `new_session` (a Resume or
 *  Restore on a paired desktop) is written for an agent ("pass
 *  over_limit_ok: true …"). The person gets the reading and where to go. */
function accountLimitText(details: unknown): string | undefined {
  const d = details as
    | { used_pct?: unknown; pause_at_pct?: unknown; suggested_email?: unknown; suggested_host?: unknown }
    | null
    | undefined;
  if (typeof d?.used_pct !== 'number') return undefined;
  const line = typeof d.pause_at_pct === 'number' ? ` (the pause line is ${Math.round(d.pause_at_pct)}%)` : '';
  const head = `This login is at ${Math.round(d.used_pct)}% of its limit${line}.`;
  if (typeof d.suggested_host !== 'string') return `${head} No other login has room right now`;
  const who = typeof d.suggested_email === 'string' && d.suggested_email.includes('@') ? d.suggested_email : 'Another login';
  return `${head} ${who} on ${d.suggested_host} has room; start it from New session to choose that login`;
}

/** Whether an `E_ACCOUNT_LIMIT` is the usage-window refusal (details carry
 *  `used_pct`), the one `over_limit_ok: true` passes, and so may offer "Start
 *  anyway". A spent daily limit (`refuse_over_daily_limit`: `daily_limit_usd`
 *  / `spent_usd`) cannot be started anyway; its fix is on the Accounts page. */
export function canStartAnyway(e: Pick<IpcError, 'code' | 'details'>): boolean {
  if (e.code !== 'E_ACCOUNT_LIMIT') return false;
  const d = e.details as { used_pct?: unknown; daily_limit_usd?: unknown } | null | undefined;
  return typeof d?.used_pct === 'number' && d.daily_limit_usd === undefined;
}

const CODE_PREFIX = /^E_[A-Z_]+:\s*/;

/** An agent connection that dropped or was replaced while a request was in
 *  flight (`agent::registry::lost_mid_request`). It travels as
 *  `E_AGENT_PROTOCOL` because `may_have_run` must count it, but it is not a
 *  version mismatch, and the person needs to know the action may have run
 *  before they retry it. */
const CONNECTION_LOST = "The connection to the host's agent dropped mid-request; it may have gone through";

function connectionLost(e: Pick<IpcError, 'details'>): boolean {
  return (e.details as { connection_lost?: unknown } | null | undefined)?.connection_lost === true;
}

function plainFor(e: Pick<IpcError, 'code' | 'details'>): string | undefined {
  if (connectionLost(e)) return CONNECTION_LOST;
  return PLAIN[e.code];
}

export interface ErrorTextOptions {
  /** The failed call was a write (an action), so a timed-out one may have
   *  run: the sentence says so. Leave it off for a read. */
  write?: boolean;
}

/** The sentence to show for `e`, never an `E_*` code. */
export function errorText(e: Pick<IpcError, 'code' | 'message' | 'details'>, opts: ErrorTextOptions = {}): string {
  const tail = opts.write === true && mayHaveRun(e);
  const plain = plainFor(e);
  if (plain) return tail ? `${plain}. ${MAY_HAVE_RUN_TAIL}` : plain;
  const msg = (e.message ?? '').replace(CODE_PREFIX, '').trim();
  const reworded = rewordedFor(e, msg);
  if (reworded) return reworded;
  const text = msg || 'Something went wrong';
  return tail ? `${text.replace(/[.!?…]$/, '')}. ${MAY_HAVE_RUN_TAIL}` : text;
}

/** This app's own hub cannot be used (`RemoteBackend::unavailable_error`:
 *  `<what> was not run: <reason> — this app is set to use a hub, …`): the
 *  reason in words. An extra fleet's refusal (`FleetRegistry::select`:
 *  `<label> (<url>) cannot be used: …`) already names the fleet and the fix,
 *  and an update check's (`no stable channel is published yet`) has nothing
 *  to do with pairing, so both keep their own words. */
function hubUnavailableText(msg: string): string | undefined {
  const m = /^.+? was not run: ([\s\S]+?)(?: — this app is set to use a hub[\s\S]*)?$/.exec(msg);
  if (!m) return undefined;
  const words = hubUnavailableWords(m[1]);
  return words.includes('Settings → Hub & sync') ? words : `${words}. Fix it in Settings → Hub & sync`;
}

/** A hub answered 401 (`HubBackend::read_result`). The hub is named by its
 *  host, not its full URL or the status code (review w16-4), because the one
 *  that refused may be a fleet added beside this app's own, whose fix is
 *  different. */
function unauthorizedText(msg: string): string {
  const m = /\((\S+) answered 401\)/.exec(msg);
  let host: string | undefined;
  if (m) {
    try {
      host = new URL(m[1]).host;
    } catch {
      host = undefined;
    }
  }
  const who = host ? `The hub at ${host}` : 'The hub';
  return `${who} no longer accepts this app's pairing. In Settings → Hub & sync, pair again if it is this app's hub, or remove the fleet and add it again with a fresh code if it is one you added`;
}

/** Review w16-10: a missing-secret refusal (`catalog::sync::apply`,
 *  `changesets::apply`) names an MCP tool and a flag an agent uses; the
 *  person has a Secrets panel. */
function secretMissingText(msg: string): string | undefined {
  const m = /^no value for (.+?);/.exec(msg);
  if (!m) return undefined;
  const several = m[1].includes(',');
  return `No value for ${m[1]}. Add ${several ? 'them' : 'it'} under Secrets, then apply again`;
}

/** Review w16-11: the two refusals a Pull or Push meets most, from git's
 *  stderr (`service::repo::repo_err`), in words. Anything else keeps git's
 *  own text. */
function gitRefusalText(msg: string): string | undefined {
  if (/\(fetch first\)|\[rejected\].*non-fast-forward|Updates were rejected because the remote/i.test(msg)) {
    return "The remote has commits this branch doesn't. Pull first";
  }
  if (/Not possible to fast-forward|diverging branches|have diverged/i.test(msg)) {
    return 'This branch and the remote have both moved; merge or rebase in the session';
  }
  return undefined;
}

/** A code whose backend message is replaced by a sentence built from it or
 *  its details; `undefined` keeps the message. */
function rewordedFor(e: Pick<IpcError, 'code' | 'details'>, msg: string): string | undefined {
  if (e.code === 'E_LOCAL_ONLY') return localOnlyText(msg);
  if (e.code === 'E_ACCOUNT_LIMIT') return accountLimitText(e.details);
  if (e.code === 'E_SECRET_MISSING') return secretMissingText(msg);
  if (e.code === 'E_REPO') return gitRefusalText(msg);
  if (e.code === 'E_HUB_UNAVAILABLE') return hubUnavailableText(msg);
  if (e.code === 'E_UNAUTHORIZED') return unauthorizedText(msg);
  return undefined;
}

/** What goes under Details: the code and, when the sentence replaced it, the
 *  original message. */
export function errorDetail(e: Pick<IpcError, 'code' | 'message' | 'details'>): string {
  const msg = (e.message ?? '').trim();
  const replaced = plainFor(e) !== undefined || rewordedFor(e, msg.replace(CODE_PREFIX, '')) !== undefined;
  return replaced && msg ? `${e.code}: ${msg}` : e.code;
}

/** `errorText` as a sentence ending in one full stop. */
export function errorSentence(e: Pick<IpcError, 'code' | 'message' | 'details'>, opts: ErrorTextOptions = {}): string {
  const t = errorText(e, opts);
  return /[.!?…]$/.test(t) ? t : `${t}.`;
}

/**
 * Why a configured hub cannot be used, in a sentence (review r13). The
 * backend's reason (`backend::Resolution::unavailable`) names settings keys
 * and keychain errors, which belong under Details.
 */
export function hubUnavailableWords(reason: string): string {
  if (/no client token is stored/.test(reason)) return "This app isn't paired with the hub yet. Pair it again from Settings → Hub & sync";
  if (/not a usable hub address/.test(reason)) return "The hub address in Settings → Hub & sync isn't a valid address";
  if (/plain ?http|plain ?text|in the clear/i.test(reason))
    return 'The hub address uses plain http to another machine, which would send this app\'s sign-in unencrypted. Use an https:// address';
  // Before the token test: `Backend::unreadable_settings` adds a clause about
  // the client token to every settings failure, even when the token is fine.
  if (/settings store|cannot read hub\./.test(reason)) return "This app couldn't read its own settings";
  if (/client token|token store/.test(reason)) return "This app can't read its saved sign-in for the hub";
  return "This app can't use the hub right now";
}
