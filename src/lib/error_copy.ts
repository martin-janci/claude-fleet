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
  E_HUB_UNAVAILABLE: "The hub isn't available",
  E_HUB_TIMEOUT: 'The hub took too long to answer',
  E_HUB_PROTOCOL: "The hub answered in a way this version doesn't understand",
  E_TIMEOUT: 'That took too long and was stopped',
  E_PTY_CLOSED: 'The terminal connection closed',
  E_PTY_BUSY: 'The terminal is busy; try again in a moment',
  E_SQLITE: "The local database couldn't do that",
  E_SERIALIZE: 'Something went wrong reading the answer',
  E_PARSE: 'Something went wrong reading the answer',
  E_INTERNAL: 'Something went wrong inside Orbit Fleet',
  E_RATE_LIMITED: 'Too many requests; try again in a moment',
};

const CODE_PREFIX = /^E_[A-Z_]+:\s*/;

/** The sentence to show for `e`, never an `E_*` code. */
export function errorText(e: Pick<IpcError, 'code' | 'message'>): string {
  const plain = PLAIN[e.code];
  if (plain) return plain;
  const msg = (e.message ?? '').replace(CODE_PREFIX, '').trim();
  return msg || 'Something went wrong';
}

/** What goes under Details: the code and, when the sentence replaced it, the
 *  original message. */
export function errorDetail(e: Pick<IpcError, 'code' | 'message'>): string {
  const msg = (e.message ?? '').trim();
  return PLAIN[e.code] && msg ? `${e.code}: ${msg}` : e.code;
}

/** `errorText` as a sentence ending in one full stop. */
export function errorSentence(e: Pick<IpcError, 'code' | 'message'>): string {
  const t = errorText(e);
  return /[.!?…]$/.test(t) ? t : `${t}.`;
}

/**
 * Why a configured hub cannot be used, in a sentence (review r13). The
 * backend's reason (`backend::Resolution::unavailable`) names settings keys
 * and keychain errors, which belong under Details.
 */
export function hubUnavailableWords(reason: string): string {
  if (/no client token is stored/.test(reason)) return "This app isn't paired with the hub yet. Pair it again from Settings → Hub";
  if (/not a usable hub address/.test(reason)) return "The hub address in Settings → Hub isn't a valid address";
  if (/plain ?text|https:\/\//i.test(reason))
    return 'The hub address uses plain http to another machine, which would send this app\'s sign-in unencrypted. Use an https:// address';
  if (/client token|token store/.test(reason)) return "This app can't read its saved sign-in for the hub";
  if (/settings store/.test(reason)) return "This app couldn't read its own settings";
  return "This app can't use the hub right now";
}
