import App from './App.svelte';
import './app.css';
import './lib/controls.css';
import { mount } from 'svelte';
import { initTheme } from './lib/theme';
import { initMotion } from './lib/motion';
import { installErrorReporting } from './lib/error_report';
import { selectedSession } from './lib/selection';
import { trackViewedSession } from './lib/session_viewed';

initTheme();
initMotion();
installErrorReporting();
trackViewedSession(selectedSession);
const app = mount(App, { target: document.getElementById('app')! });
export default app;
