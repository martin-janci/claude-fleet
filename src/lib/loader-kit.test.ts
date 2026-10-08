import { readFileSync, writeFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { extractLoaders, renderLoadersCss, renderLoadersModule } from './loader-kit-extract';

// The kit is generated from the design manual; this keeps the pair in step.
const MANUAL = 'docs/ux/2026-10-08-orbit-fleet-redesign/design-system/components';
const MODULE = 'src/lib/loader-kit.generated.ts';
const CSS = 'src/lib/loader-kit.generated.css';
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};

describe('loader kit generated from the manual', () => {
  const preview = readFileSync(`${MANUAL}/Loader/preview.html`, 'utf8');
  const bundle = readFileSync(`${MANUAL}/bundle.css`, 'utf8');
  const module = renderLoadersModule(extractLoaders(preview, bundle));
  const css = renderLoadersCss(bundle);

  it('is current', () => {
    if (env.REGEN_LOADERS) {
      writeFileSync(MODULE, module);
      writeFileSync(CSS, css);
      throw new Error(`REGEN_LOADERS: wrote ${MODULE} and ${CSS}; read the diff, then run again without it`);
    }
    expect(readFileSync(MODULE, 'utf8')).toBe(module);
    expect(readFileSync(CSS, 'utf8')).toBe(css);
  });
});
