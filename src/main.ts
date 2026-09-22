import App from './App.svelte';
import './app.css';
import './lib/controls.css';
import { mount } from 'svelte';
import { initTheme } from './lib/theme';
import { installErrorReporting } from './lib/error_report';

initTheme();
installErrorReporting();
const app = mount(App, { target: document.getElementById('app')! });
export default app;
