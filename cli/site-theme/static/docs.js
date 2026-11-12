(() => {
  'use strict';
  const body = document.body;
  const es = body.dataset.language === 'es';
  const base = new URL(body.dataset.base, location.href);
  // Zola may omit the trailing slash; assets resolve relative to a directory.
  if (!base.pathname.endsWith('/')) base.pathname += '/';
  const dialog = document.querySelector('#search-dialog');
  const input = document.querySelector('#search-input');
  const results = document.querySelector('#search-results');
  let scopesPromise;
  const scopes = () => scopesPromise ||= fetch(new URL('versions.json', base)).then(r => {
    if (!r.ok) throw new Error('Could not load documentation catalog');
    return r.json();
  }).then(data => Array.isArray(data) ? data : (data.scopes || data.versions || []));
  const currentIndex = Number(body.dataset.scope);
  const currentScope = async () => (await scopes())[currentIndex];
  document.querySelector('#theme-toggle')?.addEventListener('click', () => {
    const theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('docs-theme', theme); } catch (_) { /* Storage may be disabled. */ }
  });
  const menu = document.querySelector('#menu-toggle');
  const sidebar = document.querySelector('#sidebar');
  menu?.addEventListener('click', () => {
    const open = !sidebar?.classList.contains('is-open');
    sidebar?.classList.toggle('is-open', open);
    menu.setAttribute('aria-expanded', String(open));
  });
  document.addEventListener('click', event => {
    if (sidebar?.classList.contains('is-open') && !sidebar.contains(event.target) && !menu.contains(event.target)) {
      sidebar.classList.remove('is-open'); menu.setAttribute('aria-expanded', 'false');
    }
  });
  function notice(message) {
    const node = document.createElement('p'); node.className = 'selector-notice';
    node.setAttribute('role', 'status'); node.textContent = message; body.append(node);
    setTimeout(() => node.remove(), 7000);
  }
  try { const message = sessionStorage.getItem('docs-navigation-notice'); if (message) { sessionStorage.removeItem('docs-navigation-notice'); notice(message); } } catch (_) { /* Optional notice. */ }
  function navigateScope(target) {
    const guide = target.pages.find(page => page.slug === body.dataset.guide);
    if (!guide) {
      try { sessionStorage.setItem('docs-navigation-notice', es ? 'Esta guía no existe en la selección. Se muestra la página principal del módulo.' : 'This guide is unavailable in that selection. Showing the module overview.'); } catch (_) { /* Navigation still works without storage. */ }
    }
    location.assign(guide?.href || target.root);
  }
  document.querySelector('#version-select')?.addEventListener('change', async event => {
    try { const all = await scopes(); if (all[Number(event.target.value)]) navigateScope(all[Number(event.target.value)]); }
    catch (_) { notice(es ? 'No se pudo cargar el catálogo de versiones.' : 'The version catalog could not be loaded.'); }
  });
  document.querySelector('#language-select')?.addEventListener('change', async event => {
    try {
      const all = await scopes(); const current = all[currentIndex];
      const target = all.find(scope => scope.language === event.target.value && scope.module === current.module && scope.version === current.version);
      if (target) navigateScope(target);
      else { event.target.value = current.language; notice(es ? 'Este módulo y versión aún no están disponibles en ese idioma.' : 'This module and version are not yet available in that language.'); }
    } catch (_) { notice(es ? 'No se pudo cargar el catálogo de idiomas.' : 'The language catalog could not be loaded.'); }
  });
  document.querySelectorAll('.prose pre').forEach(pre => {
    const button = document.createElement('button'); button.className = 'copy-code'; button.type = 'button';
    const label = es ? 'Copiar' : 'Copy'; button.textContent = label;
    button.setAttribute('aria-label', es ? 'Copiar código' : 'Copy code');
    button.addEventListener('click', async () => {
      try { await navigator.clipboard.writeText(pre.querySelector('code')?.textContent || pre.textContent.replace(button.textContent, '')); button.textContent = es ? 'Copiado' : 'Copied'; }
      catch (_) { button.textContent = es ? 'No disponible' : 'Unavailable'; }
      setTimeout(() => { button.textContent = label; }, 2000);
    }); pre.append(button);
  });
  const tocLinks = [...document.querySelectorAll('.toc nav a')];
  if ('IntersectionObserver' in window) {
    const observer = new IntersectionObserver(entries => {
      for (const entry of entries) if (entry.isIntersecting) {
        tocLinks.forEach(link => link.classList.toggle('active', decodeURIComponent(new URL(link.href).hash.slice(1)) === entry.target.id));
      }
    }, {rootMargin: '-95px 0px -65% 0px'});
    document.querySelectorAll('.prose h2[id], .prose h3[id]').forEach(h => observer.observe(h));
  }
  let opener;
  const openSearch = () => { opener = document.activeElement; if (!dialog.open) dialog.showModal(); input.focus(); };
  const closeSearch = () => { dialog.close(); opener?.focus(); };
  document.querySelector('[data-search-open]')?.addEventListener('click', openSearch);
  document.querySelector('[data-search-close]')?.addEventListener('click', closeSearch);
  dialog?.addEventListener('click', event => { if (event.target === dialog) { const r = dialog.getBoundingClientRect(); if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) closeSearch(); } });
  document.addEventListener('keydown', event => {
    const editing = /INPUT|TEXTAREA|SELECT/.test(event.target.tagName) || event.target.isContentEditable;
    if ((event.key.toLowerCase() === 'k' && (event.ctrlKey || event.metaKey)) || (event.key === '/' && !editing)) { event.preventDefault(); openSearch(); }
    if (event.key === 'Escape' && sidebar?.classList.contains('is-open')) { sidebar.classList.remove('is-open'); menu.setAttribute('aria-expanded', 'false'); menu.focus(); }
  });
  const searchCache = new Map();
  async function searchEntries(language) {
    if (!searchCache.has(language)) searchCache.set(language, fetch(new URL(`search_index.${language}.json`, base)).then(r => { if (!r.ok) throw new Error('Search unavailable'); return r.json(); }).then(data => {
      const entries = Array.isArray(data) ? data : (Array.isArray(data.documents) ? data.documents : (data.documentStore?.docs ? Object.values(data.documentStore.docs) : []));
      return entries.map(item => ({title: String(item.title || ''), body: String(item.body || item.description || ''), url: String(item.url || item.id || item.ref || '')})).filter(item => item.url);
    }));
    return searchCache.get(language);
  }
  function message(text) { results.replaceChildren(); const p = document.createElement('p'); p.className = 'search-empty'; p.textContent = text; results.append(p); }
  let queryId = 0;
  input?.addEventListener('input', async () => {
    const id = ++queryId; const query = input.value.trim();
    if (query.length < 2) { message(es ? 'Escribe al menos dos caracteres.' : 'Type at least two characters.'); return; }
    try {
      const scope = body.hasAttribute('data-scope') ? await currentScope() : null;
      const entries = await searchEntries(body.dataset.language || 'en');
      if (id !== queryId) return;
      const terms = query.toLocaleLowerCase().split(/\s+/);
      const root = scope ? new URL(scope.root, location.origin).pathname.replace(/\/?$/, '/') : null;
      const matches = entries.filter(entry => {
        let path; try { path = new URL(entry.url, base).pathname.replace(/\/?$/, '/'); } catch (_) { return false; }
        return (!root || path.startsWith(root)) && terms.every(term => `${entry.title} ${entry.body}`.toLocaleLowerCase().includes(term));
      }).sort((a, b) => Number(b.title.toLocaleLowerCase().includes(query.toLocaleLowerCase())) - Number(a.title.toLocaleLowerCase().includes(query.toLocaleLowerCase()))).slice(0, 20);
      if (!matches.length) { message(es ? 'No se encontraron resultados en esta selección.' : 'No results in this selection.'); return; }
      results.replaceChildren();
      for (const entry of matches) {
        const url = new URL(entry.url, base); if (!/^https?:$/.test(url.protocol)) continue;
        const link = document.createElement('a'); link.className = 'search-result'; link.href = url.href;
        const title = document.createElement('strong'); title.textContent = entry.title;
        const excerpt = document.createElement('p'); const first = entry.body.toLocaleLowerCase().indexOf(terms[0]); const start = Math.max(0, first - 65);
        excerpt.textContent = `${start ? '…' : ''}${entry.body.slice(start, start + 190)}${entry.body.length > start + 190 ? '…' : ''}`;
        link.append(title, excerpt); results.append(link);
      }
    } catch (_) { if (id === queryId) message(es ? 'La búsqueda no está disponible. Usa la navegación para explorar las guías.' : 'Search is unavailable. Use the navigation to explore the guides.'); }
  });
  input?.addEventListener('keydown', event => { if (event.key === 'ArrowDown') { const first = results.querySelector('a'); if (first) { event.preventDefault(); first.focus(); } } });
  results?.addEventListener('keydown', event => { const links = [...results.querySelectorAll('a')]; const index = links.indexOf(document.activeElement); if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); const next = index + (event.key === 'ArrowDown' ? 1 : -1); if (next < 0) input.focus(); else links[Math.min(next, links.length - 1)]?.focus(); } });
})();
