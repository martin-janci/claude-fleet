// The views that are not on the first screen, each in its own chunk so
// launch parses only what it draws (review r16 D8). Module-level, so every
// mount shares one loader and `Lazy`'s cache imports each view once.
import { preload, type Loader } from './Lazy.svelte';

export const lazyViews = {
  hosts: () => import('./HostsView.svelte'),
  toolkit: () => import('./Toolkit.svelte'),
  accounts: () => import('./AccountsPage.svelte'),
  control: () => import('./ControlView.svelte'),
  automation: () => import('./AutomationView.svelte'),
  board: () => import('./WorkBoard.svelte'),
} satisfies Record<string, Loader>;

/** Load every view now, so a later mount renders on its first frame. */
export function preloadLazyViews(): Promise<unknown> {
  return Promise.all(Object.values(lazyViews).map((l) => preload(l)));
}
