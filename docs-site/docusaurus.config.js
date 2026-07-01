// @ts-check
// `@type` JSDoc annotations allow editor autocompletion and type checking
// (when paired with `@ts-check`).
// There are various equivalent ways to declare your Docusaurus config.
// See: https://docusaurus.io/docs/api/docusaurus-config

import {themes as prismThemes} from 'prism-react-renderer';
import {fileURLToPath} from 'node:url';
import {readFileSync} from 'node:fs';
import path from 'node:path';

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)

// --- Version sourcing -------------------------------------------------
//
// The docs display a single version number for the app. Scope decision (see
// changes/ log and docs-site README): this repo's CHANGELOG.md has no version
// headers yet — nothing has ever run `changer bump`/`changer release` to
// consolidate the per-change snippets under changes/*.md into dated,
// versioned entries. Until that happens there is no "top of CHANGELOG"
// version to read. So for this first draft we read the version straight out
// of the workspace root Cargo.toml's `[package].version`, which is the only
// real version number that exists in the repo today.
//
// TODO: once this project starts cutting releases via `changer bump` /
// `changer release` (which will populate CHANGELOG.md with real `## vX.Y.Z`
// headers), switch this to parse the top entry of ../CHANGELOG.md instead,
// and wire up Docusaurus's `docusaurus docs:version` versioned-docs feature
// to snapshot each release rather than always showing "current".
const rootDir = path.dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(path.join(rootDir, '..', 'Cargo.toml'), 'utf8');
const versionMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
const appVersion = versionMatch ? versionMatch[1] : 'unknown';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'BearCAD',
  tagline: 'Local-first, parametric CAD. Built by robots.',
  favicon: 'img/favicon.ico',

  // Future flags, see https://docusaurus.io/docs/api/docusaurus-config#future
  future: {
    v4: true, // Improve compatibility with the upcoming Docusaurus v4
  },

  // Set the production url of your site here. This is the standard
  // "https://<org>.github.io/<repo>/" shape for a GitHub Pages project site.
  url: 'https://iffy.github.io',
  // Set the /<baseUrl>/ pathname under which your site is served.
  baseUrl: '/BearCAD/',

  // GitHub pages deployment config.
  organizationName: 'iffy',
  projectName: 'BearCAD',

  onBrokenLinks: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  customFields: {
    // Injected into pages via useDocusaurusContext().siteConfig.customFields;
    // see src/components/VersionBadge.
    appVersion,
  },

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang. For example, if your site is Chinese, you
  // may want to replace "en" with "zh-Hans".
  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          sidebarPath: './sidebars.js',
          // Docs live under /docs/ so the site root (/) is free for the
          // landing page in src/pages/index.js.
          routeBasePath: '/docs',
          editUrl: 'https://github.com/iffy/BearCAD/tree/master/docs-site/',
        },
        // No blog for this first draft — the docs site is purely reference
        // material (tools/navigation + scripting), not a news feed.
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      image: 'img/logo.png',
      colorMode: {
        respectPrefersColorScheme: true,
      },
      navbar: {
        title: 'BearCAD',
        logo: {
          alt: 'BearCAD bear icon',
          src: 'img/logo.png',
        },
        items: [
          {
            type: 'doc',
            docId: 'intro',
            position: 'left',
            label: 'Overview',
          },
          {
            type: 'doc',
            docId: 'tools/index',
            position: 'left',
            label: 'Tools & Navigation',
          },
          {
            type: 'doc',
            docId: 'scripting/index',
            position: 'left',
            label: 'Scripting',
          },
          {
            href: 'https://github.com/iffy/BearCAD',
            label: 'GitHub',
            position: 'right',
          },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              {label: 'Tools & Navigation', to: '/docs/tools'},
              {label: 'Scripting', to: '/docs/scripting'},
            ],
          },
          {
            title: 'Project',
            items: [
              {label: 'GitHub', href: 'https://github.com/iffy/BearCAD'},
              {
                label: 'Releases',
                href: 'https://github.com/iffy/BearCAD/releases',
              },
            ],
          },
        ],
        // See the version-sourcing comment above: this tracks Cargo.toml
        // until the project starts cutting versioned releases via `changer`.
        copyright: `BearCAD v${appVersion} · Docs built with Docusaurus · Copyright © ${new Date().getFullYear()}`,
      },
      prism: {
        theme: prismThemes.github,
        darkTheme: prismThemes.dracula,
        additionalLanguages: ['lua', 'toml', 'bash'],
      },
    }),
};

export default config;
