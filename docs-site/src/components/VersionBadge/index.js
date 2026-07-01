import React from 'react';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';

/**
 * Small "Version X.Y.Z" badge sourced from the site's customFields.appVersion,
 * which docusaurus.config.js reads from the workspace root Cargo.toml at
 * build time. See the version-sourcing comment in docusaurus.config.js for
 * why this isn't (yet) read from CHANGELOG.md.
 */
export default function VersionBadge() {
  const {siteConfig} = useDocusaurusContext();
  const version = siteConfig.customFields?.appVersion ?? 'unknown';
  return (
    <span
      style={{
        display: 'inline-block',
        padding: '0.15rem 0.6rem',
        borderRadius: '999px',
        fontSize: '0.85rem',
        fontWeight: 600,
        border: '1px solid var(--ifm-color-emphasis-300)',
        background: 'var(--ifm-color-emphasis-100)',
        color: 'var(--ifm-color-emphasis-800)',
      }}>
      Version {version}
    </span>
  );
}
