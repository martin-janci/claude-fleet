import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import UsageBlock from './UsageBlock.svelte';
import type { AccountUsage, AccountUsageSnapshot, UsageStatus } from './account_usage_store';
import type { AccountRow } from './accounts';

// Mon 2026-09-14 14:32:00 UTC.
const NOW = 1789396320;
const MIN = 60;
const HOUR = 3600;
const RESET_5H = NOW + 38 * MIN; // 15:10
const RESET_WEEK = NOW + 2 * 86400 + 18 * HOUR + 28 * MIN; // Thu 09:00

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
    next_try_at: 0,
    ...over,
  };
}

function mount(
  props: {
    account?: AccountRow | null;
    snapshot?: AccountUsageSnapshot | null;
    sharedWith?: string[];
    now?: number;
    onRefresh?: () => void;
    suppressUnavailable?: boolean;
  } = {},
) {
  return render(UsageBlock, {
    props: {
      account,
      snapshot: snap(),
      sharedWith: [],
      now: NOW,
      locale: 'en-GB',
      timeZone: 'UTC',
      ...props,
    },
  });
}

const norm = (el: Element | null) => (el?.textContent ?? '').replace(/\s+/g, ' ').trim();
const messages = () => screen.queryAllByTestId('usage-message').map(norm);
const row = (w: string) => screen.getByTestId(`usage-row-${w}`);
const leftOf = (w: string) => norm(within(row(w)).getByTestId('usage-left'));

function expectNoSolidEmptyBar() {
  for (const m of screen.queryAllByRole('meter')) {
    // A meter without a known value must be the dashed "unknown" track.
    if (!m.hasAttribute('aria-valuenow')) expect(m).toHaveClass('unknown');
    else expect(m).not.toHaveClass('unknown');
  }
}

describe('UsageBlock — fresh', () => {
  it('renders the header, both rows, pace and the footer', () => {
    mount({ sharedWith: ['claude-fleet-oci'] });
    expect(norm(screen.getByTestId('usage-block').querySelector('header'))).toContain(
      'USAGE work · max · shared with claude-fleet-oci',
    );
    expect(screen.getByTestId('usage-account')).toHaveAttribute('title', 'admin@32bit.sk');
    expect(leftOf('5h')).toBe('91% left');
    expect(norm(within(row('5h')).getByTestId('usage-used'))).toBe('9% used');
    expect(norm(within(row('5h')).getByTestId('usage-reset'))).toBe('resets in 38 min (15:10)');
    expect(leftOf('weekly')).toBe('58% left');
    expect(norm(within(row('weekly')).getByTestId('usage-used'))).toBe('42% used');
    // 42% used with 2d 18h 28m of 7d left: 60.7% elapsed → on pace.
    expect(norm(within(row('weekly')).getByTestId('usage-reset'))).toBe(
      'resets Thu 09:00 (in 2d 18h) · on pace',
    );
    expect(norm(screen.getByTestId('usage-footer'))).toBe('via mefistos · checked 2 min ago');
    expect(messages()).toEqual([]);
    expect(screen.queryByTestId('usage-severity')).toBeNull();
  });

  it('says ahead of pace when used outruns the week', () => {
    mount({ snapshot: snap({ usage: usage({ seven_day: { utilization: 70, resets_at: RESET_WEEK } }) }) });
    expect(norm(within(row('weekly')).getByTestId('usage-reset'))).toContain('· ahead of pace');
  });

  it('carries severity by word and glyph', () => {
    mount({
      snapshot: snap({
        usage: usage({
          five_hour: { utilization: 100, resets_at: RESET_5H },
          seven_day: { utilization: 85, resets_at: RESET_WEEK },
        }),
      }),
    });
    expect(norm(within(row('5h')).getByTestId('usage-severity'))).toBe('■ LIMIT · resets 15:10');
    expect(within(row('5h')).queryByTestId('usage-reset')).toBeNull();
    expect(norm(within(row('weekly')).getByTestId('usage-severity'))).toBe('▲ LOW');
  });

  it('says EXTRA USAGE at the limit when extra usage is on, and low soon at caution', () => {
    mount({
      account: { ...account, has_extra_usage: true },
      snapshot: snap({
        usage: usage({
          five_hour: { utilization: 100, resets_at: RESET_5H },
          seven_day: { utilization: 60, resets_at: RESET_WEEK },
        }),
      }),
    });
    expect(norm(within(row('5h')).getByTestId('usage-severity'))).toBe('■ EXTRA USAGE · resets 15:10');
    expect(norm(within(row('weekly')).getByTestId('usage-severity'))).toBe('△ low soon');
  });

  it('exposes meters with used as the value and left in the label', () => {
    mount();
    const [five, week] = screen.getAllByRole('meter');
    expect(five).toHaveAttribute('aria-valuenow', '9');
    expect(five).toHaveAttribute('aria-label', '91% left');
    expect(week).toHaveAttribute('aria-valuenow', '42');
    expect(week).toHaveAttribute('aria-label', '58% left');
  });
});

describe('UsageBlock — first load and no account', () => {
  it('shows checking… with dashed tracks before the first fetch', () => {
    mount({ snapshot: null });
    expect(leftOf('5h')).toBe('checking…');
    expect(leftOf('weekly')).toBe('checking…');
    expect(messages()).toEqual(["Asking this account's hosts for usage…"]);
    const meters = screen.getAllByRole('meter');
    expect(meters).toHaveLength(2);
    for (const m of meters) {
      expect(m).toHaveClass('unknown');
      expect(m).toHaveAttribute('aria-label', 'usage unknown');
    }
    expect(screen.queryByTestId('usage-fill')).toBeNull();
    expect(screen.queryByTestId('usage-footer')).toBeNull();
  });

  it('names the host it is asking when one is known', () => {
    mount({ snapshot: snap({ status: 'never_fetched', usage: null, fetched_at: null }) });
    expect(messages()).toEqual(['Asking mefistos for usage…']);
    expectNoSolidEmptyBar();
  });

  it('a host with no account has no usage to show', () => {
    mount({ account: null, snapshot: null, onRefresh: vi.fn() });
    expect(messages()).toEqual(['Not logged in to Claude on this host — no usage to show.']);
    expect(screen.queryAllByRole('meter')).toHaveLength(0);
    expect(screen.queryByTestId('usage-refresh')).toBeNull();
  });
});

describe('UsageBlock — staleness', () => {
  it('a stale value shows ~ and its age', () => {
    mount({ snapshot: snap({ fetched_at: NOW - 14 * MIN, next_try_at: NOW + 8 * MIN }) });
    expect(leftOf('5h')).toBe('~91% left');
    expect(leftOf('weekly')).toBe('~58% left');
    expect(messages()).toEqual(['◷ 14 min old. Next try 14:40.']);
    expect(norm(screen.getByTestId('usage-footer'))).toBe('via mefistos · checked 14 min ago');
    for (const m of screen.getAllByRole('meter')) expect(m).toHaveClass('stale');
  });

  it.each<[UsageStatus, string]>([
    ['ok', '◷ 14 min old.'],
    ['rate_limited', '◷ 14 min old — last check failed: rate-limited.'],
    ['login_expired', '◷ 14 min old — last check failed: Claude login expired on mefistos.'],
    ['no_online_host', '◷ 14 min old — last check failed: no online host.'],
  ])('a stale value under status %s still shows ~ and its age', (status, age) => {
    mount({ snapshot: snap({ status, fetched_at: NOW - 14 * MIN, detail: status === 'login_expired' ? 'mefistos: login expired' : null }) });
    expect(leftOf('5h')).toMatch(/^~\d+% left$/);
    expect(messages()[0]).toBe(age);
  });

  it('an expired value shows no number', () => {
    // 45 min old: the 5-hour value is expired, the weekly one only stale.
    mount({ snapshot: snap({ fetched_at: NOW - 45 * MIN }) });
    expect(leftOf('5h')).toBe('? left');
    expect(norm(row('5h'))).not.toMatch(/\d+%/);
    expect(within(row('5h')).getByRole('meter')).toHaveClass('unknown');
    expect(leftOf('weekly')).toBe('~58% left');
    expectNoSolidEmptyBar();
  });

  it('a window past its reset withholds the number and says why', () => {
    const u = usage({ five_hour: { utilization: 92, resets_at: NOW - 22 * MIN } });
    mount({ snapshot: snap({ usage: u, fetched_at: NOW - 2 * MIN }) });
    expect(leftOf('5h')).toBe('? left');
    expect(norm(within(row('5h')).getByTestId('usage-note'))).toBe(
      'Window reset at 14:10 after the last check. Last known: 8% left at 14:30.',
    );
    expect(messages()).toEqual([]);
    expectNoSolidEmptyBar();
  });
});

describe('UsageBlock — statuses', () => {
  const noData = { usage: null, fetched_at: null } as const;
  it.each<[string, Partial<AccountUsageSnapshot>, string[]]>([
    [
      'rate_limited',
      { status: 'rate_limited', fetched_at: NOW - 2 * MIN, next_try_at: NOW + 20 * MIN, detail: 'rate limited by the usage endpoint' },
      ['⏸ Anthropic is rate-limiting usage checks. Next try 14:52.'],
    ],
    [
      'unavailable',
      { ...noData, status: 'unavailable', next_try_at: NOW + 8 * MIN, detail: 'HTTP 404: <html>nope</html>' },
      [
        "⚠ Usage unavailable. Anthropic's usage endpoint returned an unexpected response (HTTP 404). It's undocumented and may have changed. Sessions are unaffected. Next try 14:40.",
      ],
    ],
    [
      'access_token_expired',
      { ...noData, status: 'access_token_expired', detail: 'mefistos: access token expired' },
      ['🔑 Usage checks are paused until Claude Code refreshes its token on mefistos; it does so the next time it runs there.'],
    ],
    [
      'login_expired',
      { ...noData, status: 'login_expired', detail: 'claude-fleet-oci: login expired' },
      ["🔑 Claude login expired on claude-fleet-oci — usage can't be checked from it. Run claude /login there."],
    ],
    [
      'token_rejected',
      { ...noData, status: 'token_rejected', detail: 'mefistos: token rejected' },
      ["🔑 Anthropic rejected the Claude token on mefistos — usage can't be checked from it."],
    ],
    [
      'host_unsupported',
      { ...noData, status: 'host_unsupported', detail: 'mefistos: missing curl' },
      ["⚠ curl 7.55+ needed on mefistos — usage can't be checked from it."],
    ],
    [
      'no_credentials (local mac)',
      { ...noData, status: 'no_credentials', source_host: null, detail: 'local: no credentials file' },
      ['○ Usage is read through another host on this account. No other host is logged in to it.'],
    ],
    [
      'no_online_host',
      { ...noData, status: 'no_online_host', detail: 'no reachable host is logged in to this account' },
      ['○ No online host is logged in to this account (mefistos offline).'],
    ],
  ])('%s renders its exact wording and never a solid empty bar', (_name, over, expected) => {
    mount({ snapshot: snap(over) });
    expect(messages()).toEqual(expected);
    expectNoSolidEmptyBar();
    if (over.usage === null) {
      expect(leftOf('5h')).toBe('—');
      expect(leftOf('weekly')).toBe('—');
      expect(screen.queryByTestId('usage-footer')).toBeNull();
    }
  });

  it('never shows the raw detail, but offers to copy it', () => {
    mount({ snapshot: snap({ usage: null, fetched_at: null, status: 'unavailable', detail: 'HTTP 404: <html>nope</html>' }) });
    expect(norm(screen.getByTestId('usage-block'))).not.toContain('nope');
    expect(screen.getByTestId('usage-copy-details')).toHaveTextContent('Copy details');
  });

  it('drops its unavailable line and Copy details when the owner shows the banner', () => {
    const dead = snap({ fetched_at: NOW - 20 * MIN, status: 'unavailable', next_try_at: NOW + 8 * MIN, detail: 'HTTP 404: nope' });
    mount({ snapshot: dead, suppressUnavailable: true });
    expect(screen.queryAllByTestId('usage-message').map((m) => m.dataset.kind)).not.toContain('unavailable');
    expect(screen.queryByTestId('usage-copy-details')).toBeNull();
    // The age line still says why the number is old.
    expect(messages()[0]).toContain('last check failed: usage endpoint unavailable (HTTP 404)');
  });

  it('a host problem while another host answered is a muted note', () => {
    mount({ snapshot: snap({ detail: 'claude-fleet-oci: login expired' }) });
    const [note] = screen.getAllByTestId('usage-message');
    expect(norm(note)).toBe('Claude login expired on claude-fleet-oci — usage is read via mefistos. Run claude /login there.');
    expect(note).toHaveClass('tone-muted');
    expect(leftOf('5h')).toBe('91% left');
  });
});

describe('UsageBlock — refresh', () => {
  it('is disabled with a countdown while inside the floor', () => {
    const onRefresh = vi.fn();
    mount({ snapshot: snap({ next_try_at: NOW + 130 }), onRefresh });
    const btn = screen.getByTestId('usage-refresh');
    expect(btn).toBeDisabled();
    expect(norm(btn)).toBe('refresh available in 2:10');
  });

  it('is enabled once the floor has passed and calls onRefresh', async () => {
    const onRefresh = vi.fn();
    mount({ snapshot: snap({ next_try_at: NOW }), onRefresh });
    const btn = screen.getByTestId('usage-refresh');
    expect(btn).toBeEnabled();
    expect(norm(btn)).toBe('u refresh');
    await fireEvent.click(btn);
    expect(onRefresh).toHaveBeenCalledTimes(1);
  });

  it('counts down as now advances', async () => {
    const { rerender } = mount({ snapshot: snap({ next_try_at: NOW + 130 }), onRefresh: vi.fn() });
    await rerender({ now: NOW + 125 });
    expect(norm(screen.getByTestId('usage-refresh'))).toBe('refresh available in 0:05');
    await rerender({ now: NOW + 130 });
    expect(screen.getByTestId('usage-refresh')).toBeEnabled();
  });
});

describe('UsageBlock — per-model disclosure', () => {
  const bucket = (u: number) => ({ utilization: u, resets_at: RESET_WEEK });

  it('is closed by default when no bucket binds or is below 50%', async () => {
    mount({ snapshot: snap({ usage: usage({ seven_day_opus: bucket(30), seven_day_sonnet: bucket(20) }) }) });
    const btn = screen.getByTestId('usage-per-model');
    expect(norm(btn)).toBe('Per-model ▸');
    expect(btn).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('usage-per-model-rows')).toBeNull();
    expect(screen.queryByTestId('usage-binding-bucket')).toBeNull();
    await fireEvent.click(btn);
    expect(norm(btn)).toBe('Per-model ▾');
    expect(leftOf('opus')).toBe('70% left');
    expect(leftOf('sonnet')).toBe('80% left');
  });

  it('opens itself and names the bucket on the weekly line when one binds', async () => {
    mount({ snapshot: snap({ usage: usage({ seven_day_opus: bucket(71), seven_day_sonnet: bucket(20) }) }) });
    const btn = screen.getByTestId('usage-per-model');
    expect(btn).toHaveAttribute('aria-expanded', 'true');
    expect(leftOf('opus')).toBe('29% left');
    expect(norm(within(row('weekly')).getByTestId('usage-binding-bucket'))).toBe('· Opus 29% left ▲');
    await fireEvent.click(btn);
    expect(screen.queryByTestId('usage-per-model-rows')).toBeNull();
  });

  it('opens itself when a bucket is below 50% without binding', () => {
    mount({
      snapshot: snap({
        usage: usage({ seven_day: { utilization: 70, resets_at: RESET_WEEK }, seven_day_sonnet: bucket(60) }),
      }),
    });
    expect(screen.getByTestId('usage-per-model')).toHaveAttribute('aria-expanded', 'true');
    expect(screen.queryByTestId('usage-binding-bucket')).toBeNull();
  });

  it('has no disclosure without buckets', () => {
    mount();
    expect(screen.queryByTestId('usage-per-model')).toBeNull();
  });
});
