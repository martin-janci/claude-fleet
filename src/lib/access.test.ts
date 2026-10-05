import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  accessOf,
  applyGrantChanges,
  backendMode,
  myGrants,
  myPersonId,
  noAttachReason,
  parseGrantChanged,
  resetAccessForTests,
  sessionAccess,
  setMyGrants,
  type GrantLevel,
  type SessionAccess,
} from './access';
import { hubStatus, STANDALONE, unavailableReason, type HubStatus } from './hub';
import type { SessionRow } from './sessions';

/** Only the two fields the derivation reads — it takes a `Pick`, so a test
 *  need not spell out the forty-odd columns a `SessionRow` carries. */
function row(over: Partial<Pick<SessionRow, 'id' | 'owner_person_id'>> = {}) {
  return { id: 1, ...over };
}

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
};

const unavailable: HubStatus = {
  ...STANDALONE,
  unavailable: 'no stored token for https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
});

describe('backendMode', () => {
  it('names the three states the backend actually has', () => {
    expect(backendMode(STANDALONE)).toBe('local');
    expect(backendMode(remote)).toBe('remote');
    // `remote` is FALSE in this state — nothing is talking to a hub — so a
    // check that read `remote` alone would call it standalone, which is the
    // one answer that hands out `own` unconditionally.
    expect(unavailable.remote).toBe(false);
    expect(backendMode(unavailable)).toBe('unavailable');
  });
});

describe('sessionAccess', () => {
  // The table. Each row names the state it is about, because the ORDER of the
  // five rules is the thing under test and a bare input/output pair would not
  // say which rule was supposed to win.
  const cases: {
    name: string;
    status: HubStatus;
    person: number | null;
    grants: ReadonlyMap<number, GrantLevel>;
    owner: number | null | undefined;
    want: SessionAccess;
  }[] = [
    // ── rule 1: Local wins over everything ──────────────────────────────
    {
      name: 'standalone owns every row it holds, with no person id at all',
      status: STANDALONE,
      person: null,
      grants: new Map(),
      owner: undefined,
      want: 'own',
    },
    {
      name: 'standalone owns a row stamped with somebody ELSE’s person id',
      status: STANDALONE,
      person: 9,
      grants: new Map(),
      owner: 4,
      want: 'own',
    },
    // ── rule 2: fail closed, and say which problem it is ────────────────
    {
      name: 'a configured hub this launch cannot use yields nothing',
      status: unavailable,
      person: 7,
      grants: new Map([[1, 'drive' as GrantLevel]]),
      owner: 7,
      want: null,
    },
    {
      name: 'paired but with no person id yet (mid-startup) yields nothing',
      status: remote,
      person: null,
      grants: new Map(),
      owner: 7,
      want: null,
    },
    // ── rule 3: ownership, and the `strip_nulls` trap ───────────────────
    {
      name: 'paired: the row’s owner is me',
      status: remote,
      person: 7,
      grants: new Map(),
      owner: 7,
      want: 'own',
    },
    {
      name: 'paired: owner_person_id ABSENT from the frame is not ownership',
      status: remote,
      person: 7,
      grants: new Map(),
      owner: undefined,
      want: null,
    },
    {
      name: 'paired: owner_person_id null is not ownership either',
      status: remote,
      person: 7,
      grants: new Map(),
      owner: null,
      want: null,
    },
    // ── rule 4: a live grant ────────────────────────────────────────────
    {
      name: 'paired: a watch grant on somebody else’s row',
      status: remote,
      person: 7,
      grants: new Map([[1, 'watch' as GrantLevel]]),
      owner: 4,
      want: 'watch',
    },
    {
      name: 'paired: a drive grant on somebody else’s row',
      status: remote,
      person: 7,
      grants: new Map([[1, 'drive' as GrantLevel]]),
      owner: 4,
      want: 'drive',
    },
    {
      name: 'paired: a grant on a DIFFERENT session does not reach this one',
      status: remote,
      person: 7,
      grants: new Map([[2, 'drive' as GrantLevel]]),
      owner: 4,
      want: null,
    },
    // ── rule 5 ──────────────────────────────────────────────────────────
    {
      name: 'paired: somebody else’s row with no grant',
      status: remote,
      person: 7,
      grants: new Map(),
      owner: 4,
      want: null,
    },
  ];

  it.each(cases)('$name', ({ status, person, grants, owner, want }) => {
    const r = owner === undefined ? row() : row({ owner_person_id: owner });
    expect(sessionAccess(r, status, person, grants)).toBe(want);
  });

  it('answers nothing for no row at all', () => {
    expect(sessionAccess(null, STANDALONE, 1, new Map())).toBeNull();
    expect(sessionAccess(undefined, remote, 1, new Map())).toBeNull();
  });

  // The `unclaimed` row is the one visibility value besides `private`, and it
  // carries no owner — so it is exactly the shape rule 3's trap is about.
  it('an unclaimed row is own on a standalone desktop and nothing on a paired one', () => {
    const unclaimed = row({ owner_person_id: null });
    expect(sessionAccess(unclaimed, STANDALONE, null, new Map())).toBe('own');
    expect(sessionAccess(unclaimed, remote, 7, new Map())).toBeNull();
  });

  // The mid-chain state: `my_grants` and `grant:changed` do not exist in the
  // backend yet. A single-user install must keep its terminal regardless, and
  // a paired one must fail closed rather than open.
  it('keeps every single-user install working before my_grants exists', () => {
    expect(get(myPersonId)).toBeNull();
    expect(get(myGrants).size).toBe(0);
    expect(sessionAccess(row({ owner_person_id: 3 }))).toBe('own');
  });

  it('reads the stores when the optional arguments are omitted', () => {
    hubStatus.set(remote);
    setMyGrants(7, [{ session_id: 1, level: 'watch' }]);
    expect(sessionAccess(row({ owner_person_id: 4 }))).toBe('watch');
  });
});

describe('accessOf', () => {
  it('re-derives when the GRANT MAP moves and the row does not', () => {
    hubStatus.set(remote);
    setMyGrants(7, [{ session_id: 1, level: 'drive' }]);
    const r = row({ owner_person_id: 4 });
    expect(get(accessOf)(r)).toBe('drive');
    // A revoke changes no column on the row — this is the whole reason the
    // answer is derived from three inputs rather than carried on it.
    applyGrantChanges([{ session_id: 1, person_id: 7, level: null }]);
    expect(get(accessOf)(r)).toBeNull();
  });

  it('re-derives when the backend mode moves', () => {
    const r = row({ owner_person_id: 4 });
    expect(get(accessOf)(r)).toBe('own');
    hubStatus.set(remote);
    setMyGrants(7, []);
    expect(get(accessOf)(r)).toBeNull();
  });
});

describe('setMyGrants', () => {
  it('drops a level this build does not understand rather than coercing it', () => {
    // A newer hub adding a third level must read as "no grant" — no terminal,
    // no drive — not as the nearest thing we recognise.
    setMyGrants(7, [
      { session_id: 1, level: 'watch' },
      { session_id: 2, level: 'own' },
      { session_id: 3, level: 'admin' },
    ]);
    expect(get(myGrants).get(1)).toBe('watch');
    expect(get(myGrants).has(2)).toBe(false);
    expect(get(myGrants).has(3)).toBe(false);
  });
});

describe('parseGrantChanged', () => {
  it('reads a grant and a revoke', () => {
    expect(parseGrantChanged({ session_id: 3, person_id: 7, level: 'drive' })).toEqual({
      session_id: 3,
      person_id: 7,
      level: 'drive',
    });
    expect(parseGrantChanged({ session_id: 3, person_id: 7, level: null })).toEqual({
      session_id: 3,
      person_id: 7,
      level: null,
    });
  });

  it('treats a level it does not know as a revoke, not as a frame to drop', () => {
    // Dropping it would leave the old, WIDER level in the map; a grant only
    // ever moves downward, so "a level we cannot act on" is "no level".
    expect(parseGrantChanged({ session_id: 3, person_id: 7, level: 'superuser' })?.level).toBeNull();
  });

  it('rejects a malformed frame', () => {
    expect(parseGrantChanged(null)).toBeNull();
    expect(parseGrantChanged({ person_id: 7, level: 'watch' })).toBeNull();
    expect(parseGrantChanged({ session_id: '3', person_id: 7, level: 'watch' })).toBeNull();
    expect(parseGrantChanged({ session_id: 3, level: 'watch' })).toBeNull();
  });
});

describe('applyGrantChanges', () => {
  it('patches a grant to me and ignores one to somebody else', () => {
    setMyGrants(7, []);
    applyGrantChanges([
      { session_id: 1, person_id: 7, level: 'watch' },
      // A grant the OWNER made to a third person reaches the owner's stream
      // too; folding it in would be a grant to us that nobody made.
      { session_id: 2, person_id: 9, level: 'drive' },
    ]);
    expect(get(myGrants).get(1)).toBe('watch');
    expect(get(myGrants).has(2)).toBe(false);
  });

  it('narrows drive to watch', () => {
    setMyGrants(7, [{ session_id: 1, level: 'drive' }]);
    applyGrantChanges([{ session_id: 1, person_id: 7, level: 'watch' }]);
    expect(get(myGrants).get(1)).toBe('watch');
  });

  it('does nothing at all while this client has no person id', () => {
    applyGrantChanges([{ session_id: 1, person_id: 7, level: 'drive' }]);
    expect(get(myGrants).size).toBe(0);
  });
});

describe('noAttachReason', () => {
  it('has nothing to explain for a session you own', () => {
    expect(noAttachReason('own', STANDALONE)).toBeNull();
  });

  it('blames the hub when the hub is the problem, not the session', () => {
    const why = noAttachReason(null, unavailable) ?? '';
    expect(why).toContain('no stored token');
    expect(why).not.toContain('not yours');
    expect(why).not.toContain('Shared with you');
  });

  // The sentence used to be hand-copied from `hub.ts` with a comment claiming
  // it mirrored that one. Nothing held the copies together, so an edit to the
  // hub's wording left this pane saying the old thing. Identity, not
  // similarity: a re-introduced copy fails the moment `hub.ts` is edited.
  it('uses hub.ts’s own unavailable sentence rather than a copy of it', () => {
    expect(noAttachReason(null, unavailable)).toBe(unavailableReason(unavailable));
    expect(noAttachReason('watch', unavailable)).toBe(unavailableReason(unavailable));
    expect(noAttachReason('drive', unavailable)).toBe(unavailableReason(unavailable));
  });

  it('says the hub has not identified this device yet, rather than refusing', () => {
    const why = noAttachReason(null, remote) ?? '';
    expect(why).toContain('fleet.example.com');
    expect(why).toContain('who this device is');
  });

  it('explains a grant as a rule, naming the revoke that a terminal would escape', () => {
    for (const level of ['watch', 'drive'] as const) {
      const why = noAttachReason(level, remote) ?? '';
      expect(why).toContain('revoke');
      expect(why).toContain('read-only snapshot');
    }
    expect(noAttachReason('drive', remote)).toContain('drive');
  });
});
