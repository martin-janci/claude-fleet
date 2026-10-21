// Review round 13: what a person reads when a call fails. No `E_*` code and
// no transport noise in the line; the original stays reachable as Details.
import { describe, expect, it } from 'vitest';
import { canStartAnyway, errorDetail, errorSentence, errorText, hubUnavailableWords } from './error_copy';

describe('error copy', () => {
  it('turns transport failures into a sentence and keeps the original for Details', () => {
    const e = { code: 'E_SSH', message: 'ssh: connect to host mercury port 22: Connection timed out' };
    expect(errorText(e)).toBe("Couldn't reach the host over SSH");
    expect(errorDetail(e)).toBe('E_SSH: ssh: connect to host mercury port 22: Connection timed out');
  });

  // Review w16-8: these were sent and their outcome is unknown
  // (`IpcError::may_have_run`); "failed" sent the person to try again. Every
  // call says so unless it was a read (`{ read: true }`): most call sites are
  // actions, and one that forgets to opt out must not invite a second send.
  it('says a timed-out call may have gone through unless it was a read', () => {
    const e = { code: 'E_SSH_TIMEOUT', message: 'ssh mercury: timed out after 10 s' };
    expect(errorText(e)).toBe(
      'The host took too long to answer over SSH. It may have gone through anyway. The list refreshes; check it before trying again',
    );
    expect(errorText({ code: 'E_TIMEOUT', message: 'x' })).toMatch(/^That took too long to answer\. It may have gone through/);
    expect(errorText({ code: 'E_HUB_TIMEOUT', message: 'x', details: { outcome_unknown: true } })).toContain('may have gone through');
    expect(errorText({ code: 'E_HUB_TIMEOUT', message: 'x', details: { outcome_unknown: false } })).toBe('The hub took too long to answer');
    expect(errorText({ code: 'E_TIMEOUT', message: 'x' })).not.toContain('stopped');
    expect(errorSentence({ code: 'E_AGENT_PROTOCOL', message: 'x' })).toContain('It may have gone through');
    expect(errorDetail(e)).toBe('E_SSH_TIMEOUT: ssh mercury: timed out after 10 s');
    // A read (LoadError, a list refresh) gets the sentence alone.
    expect(errorText(e, { read: true })).toBe('The host took too long to answer over SSH');
    expect(errorSentence({ code: 'E_TIMEOUT', message: 'x' }, { read: true })).toBe('That took too long to answer.');
    expect(errorText({ code: 'E_AGENT_PROTOCOL', message: 'x' }, { read: true })).not.toContain('gone through');
  });

  it("keeps a backend sentence as it is, minus a leading code", () => {
    expect(errorText({ code: 'E_INVALID', message: 'E_INVALID: the name is taken' })).toBe('the name is taken');
    expect(errorDetail({ code: 'E_INVALID', message: 'the name is taken' })).toBe('E_INVALID');
    expect(errorText({ code: 'E_X', message: '' })).toBe('Something went wrong');
  });

  it('says a dropped agent connection may have run, not that the versions disagree', () => {
    const message = "the connection to mefistos's agent ended mid-request; the command may have run";
    const lost = { code: 'E_AGENT_PROTOCOL', message, details: { connection_lost: true } };
    expect(errorText(lost)).toBe("The connection to the host's agent dropped mid-request; it may have gone through");
    expect(errorText(lost)).not.toMatch(/version/);
    expect(errorDetail(lost)).toBe(`E_AGENT_PROTOCOL: ${message}`);
    // A real protocol error keeps its own sentence.
    const garbled = { code: 'E_AGENT_PROTOCOL', message: 'unexpected frame', details: { connection_lost: false } };
    expect(errorText(garbled)).toMatch(/^The host's agent answered in a way this version doesn't understand\. It may have gone through/);
  });

  // Review w16-4: the hub's 401 says it once, with no full URL or status
  // code. It names the hub's host, because the one that refused may be an
  // extra fleet, whose fix is to remove it and add it again.
  it('words a revoked pairing once, naming which hub', () => {
    const e = { code: 'E_UNAUTHORIZED', message: 'the hub revoked this client (https://extra.example:7777 answered 401) — pair again in Settings' };
    const t = errorText(e);
    expect(t).toMatch(/^The hub at extra\.example:7777 no longer accepts this app's pairing\./);
    expect(t).toContain('remove the fleet and add it again');
    expect(t).not.toContain('401');
    expect(t).not.toContain('https://');
    expect(errorDetail(e)).toContain('401');
    expect(errorText({ code: 'E_UNAUTHORIZED', message: 'nope' })).toMatch(/^The hub no longer accepts/);
  });

  it("words this app's unusable hub from its reason", () => {
    const e = {
      code: 'E_HUB_UNAVAILABLE',
      message:
        'kill_session was not run: http://10.0.0.2 sends the token in the clear. Use https:// — this app is set to use a hub, so it is managing no fleet of its own until that is fixed: pair again, or Disconnect, in Settings → Hub & sync',
    };
    const t = errorText(e);
    expect(t).toContain('plain http');
    expect(t).toContain('Settings → Hub & sync');
    expect(t).not.toContain('Pair again');
    expect(errorDetail(e)).toContain('kill_session');
  });

  // An extra fleet's refusal names the fleet and its fix; an update check's
  // is not about pairing at all. Neither may read as this app's own hub.
  it('keeps the words of an extra fleet or an update check that is unavailable', () => {
    const fleet = { code: 'E_HUB_UNAVAILABLE', message: 'company (https://co.example) cannot be used: it is not connected; fix it in Settings → Hub & sync' };
    expect(errorText(fleet)).toBe(fleet.message);
    const update = { code: 'E_HUB_UNAVAILABLE', message: 'no stable channel is published yet (nothing at https://u.example/stable.json)' };
    expect(errorText(update)).toBe(update.message);
    expect(errorText(update)).not.toMatch(/pair/i);
  });

  // Review w16-7: no command name or hub URL in front of the reason.
  it('drops the command name and the URL from a local-only refusal', () => {
    const e = {
      code: 'E_LOCAL_ONLY',
      message: 'fetch_page_source is not available while this desktop is a window onto https://hub:7777; a page reads this fleet’s store',
    };
    expect(errorText(e)).toBe('Not available on a desktop paired with a hub. A page reads this fleet’s store');
    expect(errorDetail(e)).toContain('fetch_page_source');
    // Anything else keeps its own words.
    expect(errorText({ code: 'E_LOCAL_ONLY', message: 'not from here; do it on the hub' })).toBe('not from here; do it on the hub');
  });

  // Review w16-2: written for an agent ("pass over_limit_ok"); a person gets
  // the reading and where to go.
  it('words an account-limit refusal from its details', () => {
    const message = 'a@b.com: 96% …; c@d.com has headroom (12% used): start with profile "work", or pass over_limit_ok: true once the person chose to start anyway';
    const near = { used_pct: 96.2, pause_at_pct: 90, suggested_host: 'mercury', suggested_email: 'c@d.com' };
    expect(errorText({ code: 'E_ACCOUNT_LIMIT', message, details: near })).toBe(
      'This login is at 96% of its limit (the pause line is 90%). c@d.com on mercury has room; start it from New session to choose that login',
    );
    expect(errorText({ code: 'E_ACCOUNT_LIMIT', message, details: { used_pct: 96, pause_at_pct: 90, suggested_host: null } })).toContain(
      'No other login has room',
    );
    // An account with no email shows no uuid.
    expect(errorText({ code: 'E_ACCOUNT_LIMIT', message, details: { ...near, suggested_email: '9f2c-uuid' } })).toContain('Another login on mercury');
    expect(errorDetail({ code: 'E_ACCOUNT_LIMIT', message, details: near })).toContain('over_limit_ok');
  });

  // A spent daily limit is not one `over_limit_ok` passes, so it offers no
  // "Start anyway"; the usage-window refusal does.
  it('offers Start anyway only for the usage-window refusal', () => {
    expect(canStartAnyway({ code: 'E_ACCOUNT_LIMIT', details: { used_pct: 96, pause_at_pct: 90 } })).toBe(true);
    expect(canStartAnyway({ code: 'E_ACCOUNT_LIMIT', details: { account_uuid: 'u', daily_limit_usd: 20, spent_usd: 21.5 } })).toBe(false);
    expect(canStartAnyway({ code: 'E_ACCOUNT_LIMIT', details: null })).toBe(false);
    expect(canStartAnyway({ code: 'E_INVALID', details: { used_pct: 96 } })).toBe(false);
  });

  // Review w16-10: the person has a Secrets panel, not catalog_set_secret.
  it('words a missing secret without the MCP tool or flag', () => {
    const one = errorText({ code: 'E_SECRET_MISSING', message: 'no value for GITHUB_TOKEN; set them (catalog_set_secret), then apply this card again' });
    expect(one).toBe('No value for GITHUB_TOKEN. Add it under Secrets, then apply again');
    const two = errorText({ code: 'E_SECRET_MISSING', message: 'no value for A, B; set them (catalog_set_secret) or apply again with force_partial' });
    expect(two).toBe('No value for A, B. Add them under Secrets, then apply again');
  });

  // Review w16-11: the two git refusals a Pull or Push meets most.
  it('words a rejected push and a diverged pull', () => {
    const push = errorText({
      code: 'E_REPO',
      message: " ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to 'origin'",
    });
    expect(push).toBe("The remote has commits this branch doesn't. Pull first");
    const pull = errorText({ code: 'E_REPO', message: 'fatal: Not possible to fast-forward, aborting.' });
    expect(pull).toBe('This branch and the remote have both moved; merge or rebase in the session');
    expect(errorText({ code: 'E_REPO', message: 'fatal: not a git repository' })).toBe('fatal: not a git repository');
  });

  it('ends a sentence with one full stop', () => {
    expect(errorSentence({ code: 'E_HUB_UNREACHABLE', message: 'x' })).toBe("Couldn't reach the hub.");
    expect(errorSentence({ code: 'E_INVALID', message: 'Pick a host first.' })).toBe('Pick a host first.');
  });

  it('never names a settings key when the hub cannot be used', () => {
    const reasons = [
      'hub.remote_url is not a usable hub address (relative URL without a base)',
      'http://10.0.0.2 sends the token in the clear. Use https://, or set hub.client_plaintext_token=true if …',
      'https://fleet.example.com is configured but no client token is stored',
      'cannot read the client token for https://fleet.example.com (the keychain is locked)',
      'the settings store was poisoned, and this app WAS PAIRED with a hub (a client token is stored)',
    ];
    for (const r of reasons) {
      const words = hubUnavailableWords(r);
      expect(words).not.toMatch(/hub\.\w+|keychain|poisoned|token store/);
    }
    expect(hubUnavailableWords(reasons[2])).toContain("isn't paired");
    // Review w16-5: a reason that only quotes an https:// URL is not the
    // plain-http one (it used to read as such, now that every control shows it).
    expect(hubUnavailableWords(reasons[3])).toBe("This app can't read its saved sign-in for the hub");
    expect(hubUnavailableWords(reasons[1])).toContain('plain http');
    // A settings failure is not a sign-in problem, though its reason
    // mentions the client token (`Backend::unreadable_settings`).
    expect(hubUnavailableWords(reasons[4])).toBe("This app couldn't read its own settings");
    expect(hubUnavailableWords('cannot read hub.remote_url: database is locked, and the client token store cannot be read either (x), so this app may be paired with a hub')).toBe(
      "This app couldn't read its own settings",
    );
  });
});
