import { describe, it, expect } from 'vitest';
import { deviceCode, expiryLine, linkMinutes, LOGIN_LINK_MINUTES } from './add_account';

describe('the login pane\'s device code and link expiry (M15 G7.12)', () => {
  it('reads a one-time code the pane shows, and none when it shows none', () => {
    expect(deviceCode('Open https://x/device\nEnter this one-time code: abcd-1234\n')).toBe('ABCD-1234');
    expect(deviceCode('Paste code here if prompted >')).toBeNull();
    expect(deviceCode(undefined)).toBeNull();
  });

  it('takes the expiry the pane states, else ten minutes', () => {
    expect(linkMinutes('The code expires in 15 minutes')).toBe(15);
    expect(linkMinutes('Paste code here')).toBe(LOGIN_LINK_MINUTES);
  });

  it('counts down from when the link showed, then says it expired', () => {
    const at = 1_000_000;
    expect(expiryLine(at, 10, at + 60_000)).toEqual({ text: 'The link expires in 9 min.', expired: false });
    expect(expiryLine(at, 10, at + 10 * 60_000).expired).toBe(true);
  });
});
