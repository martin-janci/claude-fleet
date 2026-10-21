// Replays the palette src/lib/theme.ts painted last (localStorage
// 'cf:theme-paint', see BOOT_KEY there) onto <html> before the stylesheet
// paints, so a launch opens on the engine's colours instead of flashing
// app.css's static first paint (review w20-5). A file, not an inline script:
// the CSP is `script-src 'self'`. theme.ts repaints for the current time and
// prefs as soon as it loads.
(function () {
  try {
    var raw = localStorage.getItem('cf:theme-paint');
    if (!raw) return;
    var snap = JSON.parse(raw);
    var root = document.documentElement;
    if (snap.theme === 'light' || snap.theme === 'dark') root.setAttribute('data-theme', snap.theme);
    var vars = snap.vars || {};
    for (var name in vars) {
      if (/^[a-z0-9-]+$/.test(name) && typeof vars[name] === 'string') root.style.setProperty('--' + name, vars[name]);
    }
  } catch (e) {
    /* no storage or a bad snapshot: app.css's first paint, as before */
  }
})();
