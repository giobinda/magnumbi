# Magnum BI website

Static info site for [Magnum BI](..). Plain HTML, CSS and a little JavaScript:
no build step, no backend, no external dependencies.

- `index.html` — landing page (features, how it works, teams, roadmap)
- `compare.html` — Magnum BI vs Tableau, Power BI, Looker, Superset, Metabase, ThoughtSpot (search landing page)
- `docs.html` — documentation (quick start, models, dashboards, connections, config, security)
- `assets/` — `style.css`, `site.js` (theme toggle, mobile menu, copy buttons), `favicon.svg`

Preview locally with `python3 -m http.server 8080 -d website` and open http://localhost:8080, or just open
`website/index.html` in a browser. It's published on GitHub Pages at
https://giobinda.github.io/magnumbi/ by `.github/workflows/website.yml` whenever `website/`
changes on `main` (one-time setup: Settings > Pages > Source: GitHub Actions). Any static host
works too, e.g. Cloudflare Pages with the root directory `website` and no build command.

Colours follow the app's default themes, Aloha and Aloha night (`backend/src/themes.rs`), a nod to
the name's origin, *Magnum, P.I.* Content is based on the repository's `README.md`; update both together when user-facing features change.

## Search engines

Each page has a title, description, canonical URL, Open Graph/Twitter tags and schema.org JSON-LD.
`robots.txt`, `sitemap.xml`, `site.webmanifest` and `assets/og-image.png` (1200×630) sit alongside.
All absolute URLs use `https://giobinda.github.io/magnumbi`. For your own domain, replace it everywhere
(and set the domain under Settings > Pages):

    grep -rl 'giobinda.github.io/magnumbi' . | xargs sed -i 's|giobinda.github.io/magnumbi|your-domain.com|g'

then submit `https://your-domain.com/sitemap.xml` in Google Search Console and Bing Webmaster Tools.
Search engines only read `robots.txt` at a domain's root, so on `github.io` it's ignored; submit
the sitemap directly instead.
Update `<lastmod>` in `sitemap.xml` when pages change.
