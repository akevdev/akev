/* Akev pre-boot theme: runs synchronously before CSS paints to avoid a theme flash.
   Mirrors resolveTheme() in src/app/theme.ts. Kept dependency-free and tiny. */
(function () {
  var root = document.documentElement;
  var theme = 'system', density = 'default', scale = 1;
  try {
    var raw = window.localStorage.getItem('akev.ui.v1');
    if (raw) {
      var prefs = JSON.parse(raw);
      if (prefs && typeof prefs === 'object') {
        if (typeof prefs.theme === 'string') theme = prefs.theme;
        if (typeof prefs.density === 'string') density = prefs.density;
        if (typeof prefs.scale === 'number') scale = prefs.scale;
      }
    }
    var override = new URLSearchParams(window.location.search).get('theme');
    if (override) theme = override;
  } catch (e) { /* storage unavailable: keep defaults */ }
  var valid = { dark: 1, light: 1, midnight: 1, contrast: 1 };
  if (!valid[theme]) {
    theme = window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  }
  root.setAttribute('data-theme', theme);
  root.setAttribute('data-density', density === 'compact' || density === 'comfortable' ? density : 'default');
  if (scale >= 0.8 && scale <= 1.4) root.style.setProperty('--ui-scale', String(scale));
  root.style.colorScheme = theme === 'light' ? 'light' : 'dark';
})();
