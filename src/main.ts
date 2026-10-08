import App from './App.svelte';
import './app.css';
import './lib/controls.css';
import './lib/loader-kit.css';
import './lib/loader-kit.generated.css';
import './lib/kit/of.generated.css';
import { mount } from 'svelte';
import { initTheme } from './lib/theme';
import { initMotion } from './lib/motion';
import { installErrorReporting } from './lib/error_report';
import { selectedSession } from './lib/selection';
import { trackViewedSession } from './lib/session_viewed';
import { startTraySync } from './lib/tray_state';
import { currentPopout } from './lib/terminal_popout';
import TerminalPopout from './lib/TerminalPopout.svelte';

initTheme();
initMotion();
installErrorReporting();
// A pop-out terminal window (redesign 5.4) is this page too, and shows one
// terminal: no tray (the main window owns it) and no "viewed" marks.
const popout = currentPopout();
const target = document.getElementById('app')!;
let app: ReturnType<typeof mount>;
if (popout) {
  app = mount(TerminalPopout, { target, props: { popout } });
} else {
  trackViewedSession(selectedSession);
  // The tray and menu-bar icon follows the fleet (redesign 3.17).
  startTraySync();
  app = mount(App, { target });
}
export default app;
