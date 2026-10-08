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

initTheme();
initMotion();
installErrorReporting();
trackViewedSession(selectedSession);
// The tray and menu-bar icon follows the fleet (redesign 3.17).
startTraySync();
const app = mount(App, { target: document.getElementById('app')! });
export default app;
