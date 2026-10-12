import { describe, expect, it } from 'vitest';
import { proposalFor, type DecisionProposal } from './proposals';

const jev: DecisionProposal = { feature: 'turn_outcome', value: 'finished', source: 'jev', confidence_pct: 82 };

describe('proposalFor', () => {
  it('finds the feature a row proposes', () => {
    expect(proposalFor({ proposals: [jev] }, 'turn_outcome')).toEqual(jev);
  });

  it('a row with no decision, an older hub, or another feature has none', () => {
    expect(proposalFor({}, 'turn_outcome')).toBeNull();
    expect(proposalFor({ proposals: null }, 'turn_outcome')).toBeNull();
    expect(proposalFor(null, 'turn_outcome')).toBeNull();
    expect(proposalFor({ proposals: [jev] }, 'start_project')).toBeNull();
  });
});
