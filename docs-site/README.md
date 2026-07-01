# BearCAD website & docs

The BearCAD website and documentation, built with [Docusaurus](https://docusaurus.io/).

- The **landing page** is served at the site root (`/`) from `src/pages/index.js`.
- The **documentation** is served under `/docs/` from the `docs/` folder.

## Install

```bash
npm ci
```

## Local development

```bash
npm run start
```

Starts a local dev server and opens a browser window. Most changes reload live.

## Build

```bash
npm run build
```

Generates the static site into `build/` — both `build/index.html` (landing page) and
`build/docs/` (documentation). Serve it locally with `npm run serve`.

## Deployment

Deployment is automated: pushes to `master` that touch `docs-site/**` trigger
[`.github/workflows/docs.yml`](../.github/workflows/docs.yml), which runs `npm run build` and
publishes `build/` to GitHub Pages.
