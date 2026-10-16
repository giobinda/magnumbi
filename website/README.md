# Magnum BI website

Static info site for [Magnum BI](..). Plain HTML, CSS and a little JavaScript:
no build step, no backend, no external dependencies.

- `index.html` — landing page (features, how it works, teams, roadmap)
- `compare.html` — Magnum BI vs Tableau, Power BI, Looker, Superset, Metabase, ThoughtSpot (search landing page)
- `docs.html` — documentation (quick start, models, dashboards, connections, config, security)
- `assets/` — `style.css`, `site.js` (theme toggle, mobile menu, copy buttons), `favicon.svg`

Preview locally with `python3 -m http.server 8080 -d website` and open http://localhost:8080, or just open
`website/index.html` in a browser. Deploy this folder to any static host: on Cloudflare Pages set the
root directory to `website` (no build command); GitHub Pages needs an Actions workflow, since it
only serves the repository root or `docs/` otherwise.

Colours follow the app's default themes, Aloha and Aloha night (`backend/src/themes.rs`), a nod to
the name's origin, *Magnum, P.I.* Content is based on the repository's `README.md`; update both together when user-facing features change.

## Search engines

Each page has a title, description, canonical URL, Open Graph/Twitter tags and schema.org JSON-LD.
`robots.txt`, `sitemap.xml`, `site.webmanifest` and `assets/og-image.png` (1200×630) sit alongside.
All absolute URLs use the placeholder `https://magnumbi.example`. Once the domain is live, replace it everywhere:

    grep -rl 'magnumbi.example' . | xargs sed -i 's|magnumbi.example|your-domain.com|g'

then submit `https://your-domain.com/sitemap.xml` in Google Search Console and Bing Webmaster Tools.
Update `<lastmod>` in `sitemap.xml` when pages change.
