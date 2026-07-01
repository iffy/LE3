import clsx from 'clsx';
import Link from '@docusaurus/Link';
import Heading from '@theme/Heading';
import styles from './styles.module.css';

const FeatureList = [
  {
    title: 'Tools & Navigation',
    to: '/docs/tools',
    description: (
      <>
        Tool-by-tool reference for Select, Sketch, Rectangle, Line, Circle, Fillet, Chamfer,
        Construction Plane, Extrude, Dimension, and Constraint — plus orbit/pan/zoom, the
        view-cube HUD, and sketch mode.
      </>
    ),
  },
  {
    title: 'Scripting',
    to: '/docs/scripting',
    description: (
      <>
        The Lua API: declarative <code>bearcad.*</code> modeling, the{' '}
        <code>bearcad.ui.*</code> simulated-interaction namespace, point-level selection, and how
        to run a script from the command line.
      </>
    ),
  },
  {
    title: 'One model, two front ends',
    to: '/docs/intro',
    description: (
      <>
        Everything achievable in the GUI is achievable by scripting, and vice versa — the same
        action layer powers the toolbar, the command palette, and the Lua API.
      </>
    ),
  },
];

function Feature({title, to, description}) {
  return (
    <div className={clsx('col col--4')}>
      <div className="text--center padding-horiz--md">
        <Heading as="h3">
          <Link to={to}>{title}</Link>
        </Heading>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
