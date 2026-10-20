// Magnum BI website: theme toggle, mobile menu, copy buttons, docs table-of-contents highlight.
(function () {
  const root = document.documentElement;
  root.classList.add('js'); // tabs and other enhancements; without it, everything shows

  // Theme: remembered per browser; falls back to the system setting.
  function getStored() { try { return localStorage.getItem('theme'); } catch (e) { return null; } }
  function store(v) { try { localStorage.setItem('theme', v); } catch (e) { /* private mode */ } }
  const saved = getStored();
  if (saved === 'light' || saved === 'dark') root.dataset.theme = saved;

  function isDark() {
    if (root.dataset.theme) return root.dataset.theme === 'dark';
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
  }
  const sun = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>';
  const moon = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/></svg>';

  document.querySelectorAll('.theme-toggle').forEach(function (btn) {
    function paint() {
      btn.innerHTML = isDark() ? sun : moon;
      btn.setAttribute('aria-label', isDark() ? 'Switch to light theme' : 'Switch to dark theme');
    }
    paint();
    btn.addEventListener('click', function () {
      const next = isDark() ? 'light' : 'dark';
      root.dataset.theme = next;
      store(next);
      paint();
    });
  });

  // Mobile menu
  const menuBtn = document.querySelector('.menu-btn');
  const nav = document.querySelector('.nav');
  if (menuBtn && nav) {
    menuBtn.addEventListener('click', function () {
      const open = nav.classList.toggle('open');
      menuBtn.setAttribute('aria-expanded', String(open));
    });
    nav.addEventListener('click', function (e) {
      if (e.target.closest('a')) { nav.classList.remove('open'); menuBtn.setAttribute('aria-expanded', 'false'); }
    });
  }


  // The setup prompt for coding agents (Claude Code, Cursor, Codex, ...): one copy, shown and copied on every page.
  const REPO_URL = 'https://github.com/giobinda/magnumbi';
  const AGENT_PROMPT = [
    'Set up Magnum BI (an open-source BI tool, currently an early alpha) on this machine and load its NYC taxi x weather demo.',
    '',
    'Source: ' + REPO_URL + '  (image: ghcr.io/giobinda/magnum-bi)',
    '',
    '1. Clone the repository, then read AGENTS.md and README.md (the "Deploying" section) and follow them.',
    '2. Check that Docker or Podman with Compose is installed and running. If not, tell me how to install it',
    '   on my OS and wait for my OK before installing anything.',
    '3. Ask me which email to use as the admin, then from the repository root run:',
    '   MAGNUM_ADMIN_EMAIL=<my email> docker compose up -d      (or: podman compose up -d)',
    '   This starts Magnum BI with its own ScramDB on http://localhost:8000.',
    '4. Find the one-time password for my first sign-in: docker compose logs magnum | grep one-time',
    '   Give it to me; I will choose my own password when I sign in.',
    '5. Load the sample data (Sales overview): docker compose exec magnum magnum-bi seed-sample',
    '   Then the demo: docker compose exec magnum magnum-bi demo --months 1',
    '   (about 60 MB of downloads per month; tell me first. More months: --months 13.)',
    '6. Tell me the URL (use localhost, not 127.0.0.1), the dashboards to open (Sales overview, nyc_taxi,',
    '   nyc_taxi_weather), and how to stop, restart and back up (the secrets volume holds the encryption key).',
    '',
    'Keep it local: do not expose it on other network interfaces or weaken any security setting.',
    'If Docker is not an option, build from source instead with the README "Quick start" (Rust 1.89+, Node 20+).',
    'Afterwards, offer to connect yourself to it over MCP: I will make an API token under Security > API tokens.'
  ].join('\n');
  document.querySelectorAll('[data-agent-prompt]').forEach(function (el) { el.textContent = AGENT_PROMPT; });
  document.querySelectorAll('[data-copy-agent-prompt]').forEach(function (btn) {
    const label = btn.innerHTML;
    btn.addEventListener('click', function () {
      if (!navigator.clipboard) return;
      navigator.clipboard.writeText(AGENT_PROMPT).then(function () {
        btn.textContent = 'Copied! Paste it into your agent';
        setTimeout(function () { btn.innerHTML = label; }, 2500);
      });
    });
  });

  // Copy buttons on code blocks
  document.querySelectorAll('.code').forEach(function (block) {
    const pre = block.querySelector('pre');
    if (!pre || !navigator.clipboard) return;
    const btn = document.createElement('button');
    btn.className = 'copy';
    btn.type = 'button';
    btn.textContent = 'Copy';
    btn.addEventListener('click', function () {
      navigator.clipboard.writeText(pre.innerText).then(function () {
        btn.textContent = 'Copied';
        setTimeout(function () { btn.textContent = 'Copy'; }, 1500);
      });
    });
    block.appendChild(btn);
  });

  // Docs: highlight the section in view
  const tocLinks = document.querySelectorAll('.toc a[href^="#"]');
  if (tocLinks.length && 'IntersectionObserver' in window) {
    const byId = {};
    tocLinks.forEach(function (a) { byId[a.getAttribute('href').slice(1)] = a; });
    const observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        tocLinks.forEach(function (a) { a.classList.remove('active'); });
        const link = byId[entry.target.id];
        if (link) link.classList.add('active');
      });
    }, { rootMargin: '-80px 0px -70% 0px' });
    Object.keys(byId).forEach(function (id) {
      const el = document.getElementById(id);
      if (el) observer.observe(el);
    });
  }

  // Footer year
  document.querySelectorAll('[data-year]').forEach(function (el) { el.textContent = new Date().getFullYear(); });

  // Docs: "In the app" / "In code" tabs. Choosing one switches every section, and is remembered.
  const groups = document.querySelectorAll('[data-tabs]');
  if (groups.length) {
    let stored = null;
    try { stored = localStorage.getItem('docs-tab'); } catch (e) { /* private mode */ }
    const choose = function (name, remember) {
      groups.forEach(function (g) {
        const tabs = g.querySelectorAll('[role="tab"]');
        const has = Array.prototype.some.call(tabs, function (t) { return t.dataset.tab === name; });
        const pick = has ? name : tabs[0].dataset.tab;
        tabs.forEach(function (t) {
          const on = t.dataset.tab === pick;
          t.setAttribute('aria-selected', String(on));
          t.tabIndex = on ? 0 : -1;
        });
        g.querySelectorAll('[role="tabpanel"]').forEach(function (p) { p.hidden = p.dataset.tab !== pick; });
      });
      if (remember) { try { localStorage.setItem('docs-tab', name); } catch (e) { /* private mode */ } }
    };
    groups.forEach(function (g, i) {
      const tabs = Array.prototype.slice.call(g.querySelectorAll('[role="tab"]'));
      tabs.forEach(function (t) {
        const panel = g.querySelector('[role="tabpanel"][data-tab="' + t.dataset.tab + '"]');
        t.id = 'tab-' + i + '-' + t.dataset.tab;
        panel.id = 'panel-' + i + '-' + t.dataset.tab;
        t.setAttribute('aria-controls', panel.id);
        panel.setAttribute('aria-labelledby', t.id);
        t.addEventListener('click', function () {
          const top = t.getBoundingClientRect().top;
          choose(t.dataset.tab, true);
          window.scrollBy(0, t.getBoundingClientRect().top - top); // stay on the section clicked
        });
        t.addEventListener('keydown', function (e) {
          const k = tabs.indexOf(t) + (e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0);
          if (k === tabs.indexOf(t)) return;
          const next = tabs[(k + tabs.length) % tabs.length];
          next.click(); next.focus(); e.preventDefault();
        });
      });
    });
    choose(stored || 'ui', false);
  }
})();
