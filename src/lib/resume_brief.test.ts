import { describe, expect, it } from 'vitest';
import { SUMMARY_HEADER, spliceSummary } from './resume_brief';

const brief = (block: string[]) =>
  [
    '# Handover: ABC-1',
    '```untrusted',
    `${SUMMARY_HEADER} (2026-10-09 08:00):`,
    ...block,
    'Title: Fix login',
    '```',
  ].join('\n');

describe('spliceSummary (review r15 F22)', () => {
  it('swaps the summary block for the edited text', () => {
    const out = spliceSummary(
      brief(['Fixed the redirect.', 'Tests red.']),
      'Fixed the redirect.\n\nTests red.',
      'Tests green now.',
    );
    expect(out).toBe(brief(['Tests green now.']));
  });

  it('drops the header and the block when the field is cleared', () => {
    const out = spliceSummary(brief(['Fixed the redirect.']), 'Fixed the redirect.', '  ');
    expect(out).toBe(['# Handover: ABC-1', '```untrusted', 'Title: Fix login', '```'].join('\n'));
  });

  it('leaves a brief alone when it no longer holds the summary as written', () => {
    expect(spliceSummary(brief(['I rewrote this.']), 'Fixed the redirect.', 'x')).toBeNull();
    expect(spliceSummary('# Handover: ABC-1', 'Fixed the redirect.', 'x')).toBeNull();
  });
});
