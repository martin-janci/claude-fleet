import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// The desktop window's Content-Security-Policy. `default-src` is the fallback
// for fetch directives only: `base-uri` and `form-action` are not fetches, so
// without their own rows an injected `<base href>` could re-point every
// relative URL, and a form a handler forgot to `preventDefault` would navigate
// the webview away from the app. `asset:` is gone from `img-src` because the
// asset protocol is not enabled — a source nothing serves is only an opening.
function directives(): Map<string, string[]> {
  const conf = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  const csp: string = conf.app.security.csp;
  const out = new Map<string, string[]>();
  for (const part of csp.split(';')) {
    const [name, ...values] = part.trim().split(/\s+/);
    if (name) out.set(name, values);
  }
  return out;
}

describe('the desktop CSP', () => {
  it('closes the directives default-src does not cover', () => {
    const d = directives();
    expect(d.get('base-uri')).toEqual(["'none'"]);
    expect(d.get('form-action')).toEqual(["'none'"]);
    expect(d.get('object-src')).toEqual(["'none'"]);
  });

  it('runs no script but its own', () => {
    expect(directives().get('script-src')).toEqual(["'self'"]);
  });

  it('allows no image source nothing serves', () => {
    expect(directives().get('img-src')).not.toContain('asset:');
  });
});
