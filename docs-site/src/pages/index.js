import clsx from 'clsx';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import HomepageFeatures from '@site/src/components/HomepageFeatures';

import Heading from '@theme/Heading';
import styles from './index.module.css';

// GitHub releases page — matches the download links in the repo README.
const RELEASES_URL = 'https://github.com/iffy/BearCAD/releases/latest';

function HomepageHeader() {
  const {siteConfig} = useDocusaurusContext();
  return (
    <header className={clsx('hero hero--primary', styles.heroBanner)}>
      <div className="container">
        <Heading as="h1" className="hero__title">
          {siteConfig.title}
        </Heading>
        <p className="hero__subtitle">{siteConfig.tagline}</p>
        <p className={styles.heroBlurb}>
          BearCAD is a local-first, parametric CAD application: a desktop GUI
          with a <code>wgpu</code>-accelerated 3D viewport, driven by a shared
          action layer that also powers a Lua scripting API. Everything you can
          do in the GUI you can do from a script, and vice versa.
        </p>
        <div className={styles.buttons}>
          <Link
            className="button button--secondary button--lg"
            to="/docs/intro">
            Read the docs
          </Link>
          <Link
            className="button button--primary button--lg"
            href={RELEASES_URL}>
            Download
          </Link>
        </div>
      </div>
    </header>
  );
}

export default function Home() {
  const {siteConfig} = useDocusaurusContext();
  return (
    <Layout
      title={siteConfig.title}
      description="BearCAD — local-first, parametric CAD with a shared GUI and Lua scripting action layer.">
      <HomepageHeader />
      <main>
        <HomepageFeatures />
      </main>
    </Layout>
  );
}
