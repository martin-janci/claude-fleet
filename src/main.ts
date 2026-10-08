import App from './App.svelte';
import './app.css';
import './lib/controls.css';
import './lib/loader-kit.css';
import './lib/loader-kit.generated.css';
import { mount } from 'svelte';
import { initTheme } from './lib/theme';
import { initMotion } from './lib/motion';
import { installErrorReporting } from './lib/error_report';

initTheme();
initMotion();
installErrorReporting();
const app = mount(App, { target: document.getElementById('app')! });
export default app;
