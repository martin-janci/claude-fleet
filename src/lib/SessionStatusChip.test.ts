// Review r15 F17: a row that Jev's reading of a silent turn moved into
// Needs you says so, with Jev's mark; a hook-set state shows nothing extra.
import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import SessionStatusChip from './SessionStatusChip.svelte';
import { session } from './hosts_fixture';

describe('SessionStatusChip: the J2 mark', () => {
  it('names Jev on a row its turn reading moved into Needs you', () => {
    render(SessionStatusChip, {
      props: {
        sess: session('h', 's', {
          claude_status: 'idle',
          turn_outcome: 'asked',
        }),
      },
    });
    expect(screen.getByTestId('jev-outcome-chip')).toHaveTextContent('Jev: asked you');
  });

  it('shows nothing extra once a hook set the state, or on the brief row', () => {
    const { unmount } = render(SessionStatusChip, {
      props: {
        sess: session('h', 's', {
          claude_status: 'blocked',
          turn_outcome: 'asked',
        }),
      },
    });
    expect(screen.queryByTestId('jev-outcome-chip')).toBeNull();
    unmount();
    render(SessionStatusChip, {
      props: {
        sess: session('h', 's', {
          claude_status: 'idle',
          turn_outcome: 'stuck',
        }),
        brief: true,
      },
    });
    expect(screen.queryByTestId('jev-outcome-chip')).toBeNull();
  });
});
