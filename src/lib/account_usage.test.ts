import { describe, it, expect } from 'vitest';
import {
  FRESH_SECS,
  bindingModelBucket,
  bindingWindow,
  checkedAgo,
  chipLabel,
  formatDuration,
  formatReset,
  formatResetShort,
  freshness,
  leftPct,
  limitWording,
  missingToolText,
  modelBuckets,
  notableBuckets,
  paceFraction,
  paceLabel,
  parseHostNotes,
  refreshCountdown,
  severity,
  severityBadge,
  statusMessage,
  usedPct,
  type UsageStatusMessage,
} from './account_usage';
import type { AccountUsage, AccountUsageSnapshot } from './account_usage_store';
import type { AccountRow } from './accounts';

// Mon 2026-09-14 14:32:00 UTC.
const NOW = 1789396320;
const MIN = 60;
const HOUR = 3600;
const L = 'en-GB';
const TZ = 'UTC';
/** 15:10 UTC — 38 minutes after NOW. */
const RESET_5H = NOW + 38 * MIN;
/** Thu 2026-09-17 09:00 UTC — 2d 18h 28m after NOW. */
const RESET_WEEK = NOW + 2 * 86400 + 18 * HOUR + 28 * MIN;

function usage(over: Partial<AccountUsage> = {}): AccountUsage {
  return {
    five_hour: { utilization: 9, resets_at: RESET_5H },
    seven_day: { utilization: 42, resets_at: RESET_WEEK },
    seven_day_opus: null,
    seven_day_sonnet: null,
    ...over,
  };
}

function snap(over: Partial<AccountUsageSnapshot> = {}): AccountUsageSnapshot {
  return {
    account_uuid: 'acct-1',
    usage: usage(),
    subscription: 'max',
    fetched_at: NOW - 2 * MIN,
    source_host: 'mefistos',
    status: 'ok',
    detail: null,
    next_try_at: NOW + 3 * MIN,
    ...over,
  };
}

const account: AccountRow = {
  uuid: 'acct-1',
  email: 'admin@32bit.sk',
  display_name: null,
  organization_name: null,
  organization_uuid: null,
  seat_tier: null,
  last_seen_at: null,
  nickname: 'work',
  has_extra_usage: false,
};

const texts = (m: UsageStatusMessage) => m.lines.map((l) => (l.glyph ? `${l.glyph} ${l.text}` : l.text));

describe('leftPct / usedPct', () => {
  it('leads with left, rounded and clamped, and used always complements it', () => {
    expect(leftPct({ utilization: 92, resets_at: null })).toBe(8);
    expect(leftPct({ utilization: 91.6, resets_at: null })).toBe(8);
    expect(leftPct({ utilization: 0, resets_at: null })).toBe(100);
    expect(leftPct({ utilization: 100, resets_at: null })).toBe(0);
    expect(leftPct({ utilization: 120, resets_at: null })).toBe(0);
    expect(leftPct({ utilization: Number.NaN, resets_at: null })).toBe(0);
    expect(usedPct({ utilization: 91.6, resets_at: null })).toBe(92);
  });
});

describe('freshness', () => {
  it('5-hour: fresh ≤ 6 min, stale ≤ 30 min, expired beyond', () => {
    const f = (age: number) => freshness('5h', NOW - age, null, NOW);
    expect(f(0)).toBe('fresh');
    expect(f(FRESH_SECS)).toBe('fresh');
    expect(f(FRESH_SECS + 1)).toBe('stale');
    expect(f(30 * MIN)).toBe('stale');
    expect(f(30 * MIN + 1)).toBe('expired');
  });

  it('weekly: fresh ≤ 6 min, stale ≤ 3 h, expired beyond', () => {
    const f = (age: number) => freshness('weekly', NOW - age, null, NOW);
    expect(f(6 * MIN)).toBe('fresh');
    expect(f(6 * MIN + 1)).toBe('stale');
    expect(f(30 * MIN + 1)).toBe('stale');
    expect(f(3 * HOUR)).toBe('stale');
    expect(f(3 * HOUR + 1)).toBe('expired');
  });

  it('is expired once now is past resets_at, and when never fetched', () => {
    expect(freshness('5h', NOW - 60, NOW - 1, NOW)).toBe('expired');
    expect(freshness('weekly', NOW - 60, NOW - 1, NOW)).toBe('expired');
    expect(freshness('5h', NOW - 60, NOW, NOW)).toBe('fresh');
    expect(freshness('5h', null, RESET_5H, NOW)).toBe('expired');
  });
});

describe('severity', () => {
  const far = NOW + 2 * HOUR;
  it('ok ≥ 50, caution 20–50, low < 20, limit at 0', () => {
    expect(severity('weekly', 100, far, NOW)).toBe('ok');
    expect(severity('weekly', 50, far, NOW)).toBe('ok');
    expect(severity('weekly', 49, far, NOW)).toBe('caution');
    expect(severity('weekly', 20, far, NOW)).toBe('caution');
    expect(severity('weekly', 19, far, NOW)).toBe('low');
    expect(severity('weekly', 1, far, NOW)).toBe('low');
    expect(severity('weekly', 0, far, NOW)).toBe('limit');
    expect(severity('5h', 0, null, NOW)).toBe('limit');
  });

  it('drops the 5-hour window one level when its reset is under 15 minutes away', () => {
    const soon = NOW + 15 * MIN - 1;
    const atFifteen = NOW + 15 * MIN;
    expect(severity('5h', 0, soon, NOW)).toBe('low');
    expect(severity('5h', 10, soon, NOW)).toBe('caution');
    expect(severity('5h', 30, soon, NOW)).toBe('ok');
    expect(severity('5h', 80, soon, NOW)).toBe('ok');
    expect(severity('5h', 0, atFifteen, NOW)).toBe('limit');
    expect(severity('5h', 10, atFifteen, NOW)).toBe('low');
  });

  it('never drops the weekly window, nor a reset already past', () => {
    expect(severity('weekly', 10, NOW + 5 * MIN, NOW)).toBe('low');
    expect(severity('5h', 10, NOW - 1, NOW)).toBe('low');
    expect(severity('5h', 10, NOW, NOW)).toBe('low');
  });

  it('extra usage changes the wording, not the level', () => {
    expect(severity('weekly', 0, far, NOW, true)).toBe('limit');
    expect(limitWording(false)).toBe('LIMIT');
    expect(limitWording(true)).toBe('EXTRA USAGE');
    expect(severityBadge('limit', true)).toEqual({ glyph: '■', word: 'EXTRA USAGE' });
    expect(severityBadge('limit', false)).toEqual({ glyph: '■', word: 'LIMIT' });
    expect(severityBadge('low', false)).toEqual({ glyph: '▲', word: 'LOW' });
    expect(severityBadge('caution', false)).toEqual({ glyph: '△', word: 'low soon' });
    expect(severityBadge('ok', false)).toBeNull();
  });
});

describe('bindingWindow and model buckets', () => {
  it('picks the window with fewer % left, 5-hour on a tie', () => {
    expect(bindingWindow(usage())).toBe('weekly');
    expect(bindingWindow(usage({ five_hour: { utilization: 92, resets_at: RESET_5H } }))).toBe('5h');
    expect(bindingWindow(usage({ five_hour: { utilization: 10, resets_at: RESET_5H } }))).toBe('weekly');
    expect(
      bindingWindow(usage({ five_hour: { utilization: 42, resets_at: RESET_5H } })),
    ).toBe('5h');
  });

  it('handles missing buckets', () => {
    expect(bindingWindow(usage({ five_hour: null }))).toBe('weekly');
    expect(bindingWindow(usage({ seven_day: null }))).toBe('5h');
    expect(bindingWindow(usage({ five_hour: null, seven_day: null }))).toBeNull();
    expect(bindingWindow(null)).toBeNull();
    expect(modelBuckets(null)).toEqual([]);
    expect(modelBuckets(usage())).toEqual([]);
    expect(bindingModelBucket(usage())).toBeNull();
  });

  it('names a model bucket only when it has fewer % left than overall weekly', () => {
    const u = usage({
      seven_day: { utilization: 42, resets_at: RESET_WEEK }, // 58% left
      seven_day_opus: { utilization: 71, resets_at: RESET_WEEK }, // 29% left
      seven_day_sonnet: { utilization: 30, resets_at: RESET_WEEK }, // 70% left
    });
    expect(bindingModelBucket(u)).toMatchObject({ model: 'Opus', left: 29, binds: true });
    expect(notableBuckets(u).map((b) => b.model)).toEqual(['Opus']);
    const equal = usage({ seven_day_sonnet: { utilization: 42, resets_at: RESET_WEEK } });
    expect(bindingModelBucket(equal)).toBeNull();
    // Below 50% left is notable even when it does not bind.
    const bothLow = usage({
      seven_day: { utilization: 70, resets_at: RESET_WEEK },
      seven_day_sonnet: { utilization: 60, resets_at: RESET_WEEK },
    });
    expect(bindingModelBucket(bothLow)).toBeNull();
    expect(notableBuckets(bothLow).map((b) => b.model)).toEqual(['Sonnet']);
    // With no overall weekly figure nothing can bind.
    expect(bindingModelBucket(usage({ seven_day: null, seven_day_opus: { utilization: 90, resets_at: null } }))).toBeNull();
  });
});

describe('formatReset', () => {
  it('5-hour reads countdown then clock', () => {
    expect(formatReset('5h', RESET_5H, NOW, L, TZ)).toBe('resets in 38 min (15:10)');
    expect(formatReset('5h', NOW + 59, NOW, L, TZ)).toBe('resets in <1 min (14:32)');
    expect(formatReset('5h', NOW + 60, NOW, L, TZ)).toBe('resets in 1 min (14:33)');
    expect(formatReset('5h', NOW + 4 * HOUR + 5 * MIN, NOW, L, TZ)).toBe('resets in 4h 5m (18:37)');
  });

  it('weekly reads weekday clock then countdown', () => {
    expect(formatReset('weekly', RESET_WEEK, NOW, L, TZ)).toBe('resets Thu 09:00 (in 2d 18h)');
    expect(formatReset('weekly', NOW + 5 * HOUR, NOW, L, TZ)).toBe('resets Mon 19:32 (in 5h 0m)');
    expect(formatReset('weekly', NOW + 30, NOW, L, TZ)).toBe('resets Mon 14:32 (in <1 min)');
  });

  it('handles a missing or past reset', () => {
    expect(formatReset('5h', null, NOW, L, TZ)).toBe('reset time unknown');
    expect(formatReset('weekly', null, NOW, L, TZ)).toBe('reset time unknown');
    expect(formatReset('5h', NOW - 10 * MIN, NOW, L, TZ)).toBe('reset at 14:22');
    expect(formatResetShort('5h', RESET_5H, NOW, L, TZ)).toBe('resets 15:10');
    expect(formatResetShort('weekly', RESET_WEEK, NOW, L, TZ)).toBe('resets Thu 09:00');
    expect(formatResetShort('weekly', null, NOW, L, TZ)).toBe('reset time unknown');
  });

  it('formats durations and ages', () => {
    expect(formatDuration(-5)).toBe('<1 min');
    expect(formatDuration(59)).toBe('<1 min');
    expect(formatDuration(14 * MIN)).toBe('14 min');
    expect(formatDuration(2 * HOUR + 10 * MIN)).toBe('2h 10m');
    expect(checkedAgo(NOW - 2 * MIN, NOW)).toBe('checked 2 min ago');
    expect(checkedAgo(NOW - 20, NOW)).toBe('checked just now');
  });

  it('counts the refresh floor down as m:ss', () => {
    expect(refreshCountdown(NOW + 130, NOW)).toBe('2:10');
    expect(refreshCountdown(NOW + 5, NOW)).toBe('0:05');
    expect(refreshCountdown(NOW, NOW)).toBeNull();
    expect(refreshCountdown(0, NOW)).toBeNull();
  });
});

describe('pace', () => {
  it('is the fraction of the 7-day window elapsed, clamped', () => {
    const reset = NOW + 3.5 * 86400;
    expect(paceFraction(reset, NOW)).toBeCloseTo(0.5);
    expect(paceFraction(NOW + 7 * 86400, NOW)).toBe(0);
    expect(paceFraction(NOW + 8 * 86400, NOW)).toBe(0);
    expect(paceFraction(NOW - 10, NOW)).toBe(1);
    expect(paceFraction(null, NOW)).toBeNull();
  });

  it('is on pace while used ≤ elapsed', () => {
    expect(paceLabel(0.4, 0.5)).toBe('on pace');
    expect(paceLabel(0.5, 0.5)).toBe('on pace');
    expect(paceLabel(0.51, 0.5)).toBe('ahead of pace');
    expect(paceLabel(0.5, null)).toBeNull();
  });
});

describe('chipLabel', () => {
  it('shows % left and the reset together, naming the window only when weekly binds', () => {
    expect(chipLabel(snap(), NOW, false, L, TZ)).toBe('weekly 58% left · resets Thu 09:00');
    const fiveBinds = snap({ usage: usage({ five_hour: { utilization: 60, resets_at: RESET_5H } }) });
    expect(chipLabel(fiveBinds, NOW, false, L, TZ)).toBe('40% left · resets 15:10');
  });

  it('marks stale with ~ and ◷, and expired with ? left', () => {
    const five = usage({
      five_hour: { utilization: 38, resets_at: RESET_5H },
      seven_day: { utilization: 10, resets_at: RESET_WEEK },
    });
    expect(chipLabel(snap({ usage: five, fetched_at: NOW - 14 * MIN }), NOW, false, L, TZ)).toBe(
      '~62% left ◷ · resets 15:10',
    );
    expect(chipLabel(snap({ usage: five, fetched_at: NOW - 31 * MIN }), NOW, false, L, TZ)).toBe('? left');
    const past = { ...five, five_hour: { utilization: 38, resets_at: NOW - 1 } };
    expect(chipLabel(snap({ usage: past, fetched_at: NOW - 2 * MIN }), NOW, false, L, TZ)).toBe('? left');
  });

  it('carries low and limit by glyph and word', () => {
    const low = usage({ five_hour: { utilization: 92, resets_at: RESET_5H } });
    expect(chipLabel(snap({ usage: low }), NOW, false, L, TZ)).toBe('▲ 8% left · resets 15:10');
    const limit = usage({ five_hour: { utilization: 100, resets_at: RESET_5H } });
    expect(chipLabel(snap({ usage: limit }), NOW, false, L, TZ)).toBe('■ LIMIT · resets 15:10');
    expect(chipLabel(snap({ usage: limit }), NOW, true, L, TZ)).toBe('■ EXTRA USAGE · resets 15:10');
    const weekLimit = usage({ seven_day: { utilization: 100, resets_at: RESET_WEEK } });
    expect(chipLabel(snap({ usage: weekLimit }), NOW, false, L, TZ)).toBe('■ weekly LIMIT · resets Thu 09:00');
  });

  it('says checking… before the first fetch and ? left with nothing usable', () => {
    expect(chipLabel(null, NOW)).toBe('checking…');
    expect(chipLabel(snap({ status: 'never_fetched', usage: null, fetched_at: null }), NOW)).toBe('checking…');
    expect(chipLabel(snap({ status: 'login_expired', usage: null, fetched_at: null }), NOW)).toBe('? left');
    expect(chipLabel(snap({ usage: usage({ five_hour: null, seven_day: null }) }), NOW)).toBe('? left');
    const noReset = usage({ five_hour: { utilization: 60, resets_at: null } });
    expect(chipLabel(snap({ usage: noReset }), NOW, false, L, TZ)).toBe('40% left');
  });
});

describe('parseHostNotes and missingToolText', () => {
  it('classifies each per-host note', () => {
    const notes = parseHostNotes(
      'local: no credentials file; mefistos: login expired; oci: missing curl; htz: exit 255: ssh: connect to host htz port 22: timed out',
    );
    expect(notes.map((n) => [n.host, n.problem])).toEqual([
      ['local', 'no_credentials'],
      ['mefistos', 'login_expired'],
      ['oci', 'host_unsupported'],
      ['htz', 'unreachable'],
    ]);
    expect(parseHostNotes(null)).toEqual([]);
    expect(parseHostNotes('rate limited by the usage endpoint')).toEqual([]);
  });

  it('names what is missing', () => {
    expect(missingToolText('missing curl', 'oci')).toBe('curl 7.55+ needed on oci');
    expect(missingToolText('curl is older than 7.55', 'oci')).toBe('curl 7.55+ needed on oci');
    expect(missingToolText('missing python3_or_jq', 'oci')).toBe('python3 or jq needed on oci');
    expect(missingToolText('missing mktemp', 'oci')).toBe('mktemp needed on oci');
    expect(missingToolText(null, 'oci')).toBe('curl 7.55+ or python3/jq needed on oci');
  });
});

describe('statusMessage', () => {
  const msg = (s: AccountUsageSnapshot | null, shared: string[] = [], acct: AccountRow | null = account) =>
    statusMessage(s, acct, shared, NOW, L, TZ);

  it('host with no account', () => {
    expect(texts(msg(snap(), [], null))).toEqual(['Not logged in to Claude on this host — no usage to show.']);
  });

  it('first load', () => {
    const m = msg(snap({ status: 'never_fetched', usage: null, fetched_at: null, source_host: 'mefistos' }));
    expect(m.checking).toBe(true);
    expect(texts(m)).toEqual(['Asking mefistos for usage…']);
    expect(texts(msg(null))).toEqual(["Asking this account's hosts for usage…"]);
    expect(msg(null).checking).toBe(true);
  });

  it('fresh ok has no lines', () => {
    const m = msg(snap());
    expect(m.lines).toEqual([]);
    expect(m.checking).toBe(false);
  });

  it('stale ok shows its age and the next try', () => {
    expect(texts(msg(snap({ fetched_at: NOW - 14 * MIN, next_try_at: NOW + 8 * MIN })))).toEqual([
      '◷ 14 min old. Next try 14:40.',
    ]);
    expect(texts(msg(snap({ fetched_at: NOW - 14 * MIN, next_try_at: 0 })))).toEqual(['◷ 14 min old.']);
  });

  it('expired past reset names the reset and the last known value', () => {
    const u = usage({ five_hour: { utilization: 92, resets_at: NOW - 22 * MIN } }); // reset 14:10
    const m = msg(snap({ usage: u, fetched_at: NOW - 2 * MIN, next_try_at: 0 }));
    expect(texts(m)).toEqual(['Window reset at 14:10 after the last check. Last known: 8% left at 14:30.']);
    expect(m.lines[0].window).toBe('5h');
  });

  it('rate-limited', () => {
    const m = msg(snap({ status: 'rate_limited', fetched_at: NOW - 14 * MIN, next_try_at: NOW + 20 * MIN, detail: 'rate limited by the usage endpoint (Retry-After: 900s)' }));
    expect(texts(m)).toEqual([
      '◷ 14 min old — last check failed: rate-limited.',
      '⏸ Anthropic is rate-limiting usage checks. Next try 14:52.',
    ]);
    expect(m.copyDetail).toBe('rate limited by the usage endpoint (Retry-After: 900s)');
  });

  it('endpoint unavailable, with the HTTP status but never the raw body', () => {
    const detail = 'HTTP 404: {"error":"not_found"}';
    const m = msg(snap({ status: 'unavailable', detail, fetched_at: NOW - 14 * MIN, next_try_at: NOW + 8 * MIN }));
    expect(texts(m)).toEqual([
      '◷ 14 min old — last check failed: usage endpoint unavailable (HTTP 404).',
      "⚠ Usage unavailable. Anthropic's usage endpoint returned an unexpected response (HTTP 404). It's undocumented and may have changed. Sessions are unaffected. Next try 14:40.",
    ]);
    expect(texts(m).join(' ')).not.toContain('not_found');
    expect(m.copyDetail).toBe(detail);
  });

  it('access token expired is benign: no login instruction', () => {
    const m = msg(snap({ status: 'access_token_expired', usage: null, fetched_at: null, detail: 'mefistos: access token expired' }));
    expect(texts(m)).toEqual([
      '🔑 Usage checks are paused until Claude Code refreshes its token on mefistos; it does so the next time it runs there.',
    ]);
    expect(texts(m).join(' ')).not.toContain('/login');
    expect(m.lines[0].tone).toBe('muted');
  });

  it('login expired asks for claude /login on that host', () => {
    const m = msg(snap({ status: 'login_expired', usage: null, fetched_at: null, source_host: null, detail: 'oci: login expired' }));
    expect(texts(m)).toEqual([
      "🔑 Claude login expired on oci — usage can't be checked from it. Run claude /login there.",
    ]);
    expect(m.lines[0].tone).toBe('alarm');
  });

  it('login expired while stale carries the reason and next try on the age line', () => {
    const m = msg(snap({ status: 'login_expired', fetched_at: NOW - 14 * MIN, detail: 'mefistos: login expired', next_try_at: NOW + 8 * MIN }));
    expect(texts(m)[0]).toBe('◷ 14 min old — last check failed: Claude login expired on mefistos. Next try 14:40.');
  });

  it('token rejected', () => {
    const m = msg(snap({ status: 'token_rejected', usage: null, fetched_at: null, detail: 'mefistos: token rejected' }));
    expect(texts(m)).toEqual(["🔑 Anthropic rejected the Claude token on mefistos — usage can't be checked from it."]);
    expect(texts(m).join(' ')).not.toContain('/login');
  });

  it('host unsupported names what is missing', () => {
    const m = msg(snap({ status: 'host_unsupported', usage: null, fetched_at: null, detail: 'oci: missing python3_or_jq' }));
    expect(texts(m)).toEqual(["⚠ python3 or jq needed on oci — usage can't be checked from it."]);
  });

  it('no credentials on the local mac: read through another host', () => {
    const m = msg(snap({ status: 'no_credentials', usage: null, fetched_at: null, source_host: null, detail: 'local: no credentials file' }));
    expect(texts(m)).toEqual(['○ Usage is read through another host on this account. No other host is logged in to it.']);
    const shared = msg(snap({ status: 'no_credentials', usage: null, fetched_at: null, source_host: null, detail: 'local: no credentials file' }), ['trn']);
    expect(texts(shared)).toEqual(['○ Usage is read through another host on this account. None of trn could be asked.']);
  });

  it('no credentials on a remote host', () => {
    const m = msg(snap({ status: 'no_credentials', usage: null, fetched_at: null, detail: 'htz: no credentials file' }));
    expect(texts(m)).toEqual(["🔑 No Claude credentials on htz — usage can't be checked from it."]);
  });

  it('no online host keeps last-known values under the rules', () => {
    const m = msg(snap({ status: 'no_online_host', fetched_at: NOW - 14 * MIN, next_try_at: 0, detail: 'no reachable host is logged in to this account' }));
    expect(texts(m)).toEqual([
      '◷ 14 min old — last check failed: no online host.',
      '○ No online host is logged in to this account (mefistos offline).',
    ]);
  });

  it('unreachable hosts after a transport failure', () => {
    const m = msg(snap({ status: 'no_online_host', usage: null, fetched_at: null, source_host: null, detail: 'htz: exit 255: ssh: connect to host htz port 22: Operation timed out' }));
    expect(texts(m)).toEqual(["○ Couldn't reach htz to check usage."]);
  });

  it('a host problem while another host answered is a muted note', () => {
    const m = msg(snap({ detail: 'oci: login expired; local: no credentials file; htz: missing curl' }));
    expect(texts(m)).toEqual([
      'Claude login expired on oci — usage is read via mefistos. Run claude /login there.',
      'curl 7.55+ needed on htz — usage is read via mefistos.',
    ]);
    expect(m.lines.every((l) => l.tone === 'muted')).toBe(true);
  });
});
