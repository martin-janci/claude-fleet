import App from './App.svelte';
import './app.css';
import { mount } from 'svelte';
import { initTheme } from './lib/theme';

initTheme();
const app = mount(App, { target: document.getElementById('app')! });
export default app;
