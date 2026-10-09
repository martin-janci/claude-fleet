// Review round 13: what a person reads when a call fails. No `E_*` code and
// no transport noise in the line; the original stays reachable as Details.
import { describe, expect, it } from 'vitest';
import { errorDetail, errorSentence, errorText, hubUnavailableWords } from './error_copy';

describe('error copy', () => {
  it('turns transport failures into a sentence and keeps the original for Details', () => {
    const e = { code: 'E_SSH_TIMEOUT', message: 'ssh mercury: timed out after 10 s' };
    expect(errorText(e)).toBe('The host took too long to answer over SSH');
    expect(errorDetail(e)).toBe('E_SSH_TIMEOUT: ssh mercury: timed out after 10 s');
  });

  it("keeps a backend sentence as it is, minus a leading code", () => {
    expect(errorText({ code: 'E_INVALID', message: 'E_INVALID: the name is taken' })).toBe('the name is taken');
    expect(errorDetail({ code: 'E_INVALID', message: 'the name is taken' })).toBe('E_INVALID');
    expect(errorText({ code: 'E_X', message: '' })).toBe('Something went wrong');
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
  });
});
