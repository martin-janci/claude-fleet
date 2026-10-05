// The per-session action predicate (multi-user M1, F2). `access.test.ts` pins
// the derivation this reads; what is under test here is the LEVEL TABLE — that
// `own` means the spec §4.3 tier and nothing wider, that `drive` is not a
// licence to kill, and that "we could not tell" blames the hub rather than the
// session.
import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { resetAccessForTests, setMyGrants, type SessionAccess } from './access';
import { hubStatus, STANDALONE, unavailableReason, type HubStatus } from './hub';
import { sessionActionBlocked, sessionBlocked, shareSheetFor, type SessionAction } from './share';
import type { SessionRow } from './sessions';

/** Only the two fields the predicate's derivation reads. */
const row = (over: Partial<Pick<SessionRow, 'id' | 'owner_person_id'>> = {}) => ({
  id: 1,
  owner_person_id: 7,
  ...over,
});

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

const unavailable: HubStatus = {
  ...STANDALONE,
  unavailable: 'no stored token for https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
  shareSheetFor.set(null);
});

describe('the unavailable sentence has one home', () => {
  // `hub.ts::unavailableReason` is the only copy: this predicate and
  // `access.ts::noAttachReason` both call it. Three hand-copies were three
  // chances for an edit to the hub's wording to leave two surfaces stale.
  it('is hub.ts’s, called and not copied', () => {
    expect(sessionActionBlocked(row(), 'send_prompt', null, unavailable)).toBe(
      unavailableReason(unavailable),
    );
    expect(sessionActionBlocked(row(), 'kill_session', null, unavailable)).toBe(
      unavailableReason(unavailable),
    );
  });
});

describe('sessionActionBlocked', () => {
  /** One action from each tier, named so a failure says which rule moved. */
  const OWN: SessionAction[] = [
    'kill_session',
    'safe_kill_session',
    'restart_session',
    'recreate_session',
    'move_session',
    'spawn_review',
    'rewind_conversation',
    'rename_session',
    'set_session_tags',
    'summarize_past_work',
    'session_share',
    'session_unshare',
    'session_narrow',
    'restore_host_sessions',
  ];
  const DRIVE: SessionAction[] = [
    'send_prompt',
    'send_message',
    'dispatch_task',
    'set_friendly_name',
    'link_session_work',
  ];
  const READ: SessionAction[] = ['capture_session', 'session_conversation', 'session_history'];

  it('blocks nothing at all for the owner', () => {
    for (const a of [...OWN, ...DRIVE, ...READ]) {
      expect(sessionActionBlocked(row(), a, 'own', remote)).toBeNull();
    }
  });

  it('a drive grantee may work the session but not dispose of it (spec §4.3 invariant 5)', () => {
    for (const a of DRIVE) expect(sessionActionBlocked(row(), a, 'drive', remote)).toBeNull();
    for (const a of READ) expect(sessionActionBlocked(row(), a, 'drive', remote)).toBeNull();
    for (const a of OWN) {
      const why = sessionActionBlocked(row(), a, 'drive', remote);
      expect(why, a).not.toBeNull();
      expect(why).toMatch(/only the session’s owner/i);
    }
  });

  it('a watch grantee may read and nothing else', () => {
    for (const a of READ) expect(sessionActionBlocked(row(), a, 'watch', remote)).toBeNull();
    for (const a of DRIVE) {
      const why = sessionActionBlocked(row(), a, 'watch', remote);
      expect(why, a).not.toBeNull();
      expect(why).toMatch(/needs drive/i);
    }
    for (const a of OWN) expect(sessionActionBlocked(row(), a, 'watch', remote)).not.toBeNull();
  });

  it('never says "not yours" when the truth is that the hub is unreachable', () => {
    const why = sessionActionBlocked(row(), 'kill_session', null, unavailable);
    expect(why).toContain('no stored token');
    expect(why).not.toMatch(/owner/i);
  });

  it('says the hub has not identified this device yet, which is a different problem', () => {
    const why = sessionActionBlocked(row(), 'kill_session', null, remote);
    expect(why).toContain('https://fleet.example.com');
    expect(why).toMatch(/has not said who this device is/i);
  });

  it('tells "somebody else’s session" apart from "we could not tell"', () => {
    // Three states, three sentences. The hub fences a stranger's row off the
    // stream, so the third is not supposed to be reachable — but when it is,
    // sending someone to Settings → Hub over it would be wrong.
    expect(sessionActionBlocked(row(), 'send_prompt', null, remote, 4)).toMatch(
      /belongs to someone else/i,
    );
    expect(sessionActionBlocked(row(), 'send_prompt', null, remote, null)).toMatch(
      /has not said who this device is/i,
    );
    expect(sessionActionBlocked(row(), 'send_prompt', null, unavailable, 4)).toContain(
      'no stored token',
    );
  });

  it('blocks nothing when there is no session to block it on', () => {
    expect(sessionActionBlocked(null, 'kill_session', null, remote)).toBeNull();
    expect(sessionActionBlocked(undefined, 'kill_session', 'watch', remote)).toBeNull();
  });

  it('reads the stores when the access argument is omitted — standalone owns everything', () => {
    // The single-user case, which must keep working with no `my_grants` answer
    // at all: `access.ts`'s rule 1 short-circuits on the backend mode.
    expect(sessionActionBlocked(row(), 'kill_session')).toBeNull();
  });
});

describe('sessionBlocked (the store)', () => {
  it('re-answers when the GRANT MAP moves and the row does not', () => {
    // The assertion that would have failed under a per-caller field on the
    // row: narrowing a grant changes no column on any session, so a predicate
    // that read the row alone would leave the drive actions enabled.
    hubStatus.set(remote);
    setMyGrants(3, [{ session_id: 1, level: 'drive' }]);
    expect(get(sessionBlocked)(row({ owner_person_id: 9 }), 'send_prompt')).toBeNull();

    setMyGrants(3, [{ session_id: 1, level: 'watch' }]);
    expect(get(sessionBlocked)(row({ owner_person_id: 9 }), 'send_prompt')).toMatch(/needs drive/i);

    setMyGrants(3, []);
    // Revoked entirely: the sentence names the real state — somebody else's
    // session — rather than blaming the hub the way "we could not tell" does.
    expect(get(sessionBlocked)(row({ owner_person_id: 9 }), 'send_prompt')).toMatch(
      /belongs to someone else/i,
    );
  });

  it('re-answers when the backend mode moves', () => {
    const r = row({ owner_person_id: 9 });
    expect(get(sessionBlocked)(r, 'kill_session')).toBeNull(); // standalone
    hubStatus.set(remote);
    setMyGrants(3, []);
    expect(get(sessionBlocked)(r, 'kill_session')).not.toBeNull();
  });
});

describe('the own tier is the spec’s list, not a superset of it', () => {
  // A regression guard with a point: it is tempting to put every write in
  // `own` "to be safe", which would take `send_prompt` away from a driver and
  // make the drive level meaningless. These four are the ones revision 4's
  // three disagreeing copies of the list argued over.
  const DRIVER_MAY: SessionAction[] = [
    'send_prompt',
    'send_message',
    'set_friendly_name',
    'link_session_work',
  ];
  const DRIVER_MAY_NOT: SessionAction[] = ['kill_session', 'restart_session', 'rename_session'];

  it('a driver sends prompts and messages; a driver does not kill, restart or rename', () => {
    const access: SessionAccess = 'drive';
    for (const a of DRIVER_MAY) expect(sessionActionBlocked(row(), a, access, remote)).toBeNull();
    for (const a of DRIVER_MAY_NOT)
      expect(sessionActionBlocked(row(), a, access, remote)).not.toBeNull();
  });
});
