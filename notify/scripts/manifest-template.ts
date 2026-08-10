/**
 * Static manifest fields. `version` (from this repo's package.json) and
 * `files` (sha256 of the built bundle + logo) are filled in by
 * `scripts/build.ts` — this template is not the manifest itself.
 *
 * `fliks` was derived by reading `backend/package.json`'s `version` in the
 * Fliks repo at authoring time (currently "2.0.1") and picking the loosest
 * range with a mandatory upper bound that still covers it: the whole 2.x
 * line, cut off before an unknown-shape 3.0.
 */
export const PLUGIN_ID = 'fliks.notify';

/**
 * Single source of truth for this plugin's own policy strings — not core
 * CASL subjects. Core's plugin-declared permission registry/namespacing is
 * still being designed in a concurrent change; keeping every reference here
 * means reconciling it later is a one-line edit, not a hunt through routes.
 * Two rights, deliberately not one: reading the log is inert, sending a
 * test delivery makes an outbound network call and is the privileged one.
 */
export const POLICY = {
  deliveriesRead: `${PLUGIN_ID}:deliveries.read`,
  deliveriesTest: `${PLUGIN_ID}:deliveries.test`,
} as const;

export const MANIFEST_TEMPLATE = {
  id: PLUGIN_ID,
  pluginApi: 0,
  name: 'Notify',
  fliks: '>=2.0.0 <3.0.0',
  author: 'Fliks',
  description: 'Forwards Fliks domain events to an admin-configured HTTPS webhook, with bounded retry.',
  license: 'MIT',
  logo: 'logo.svg',
  kind: 'process' as const,
  runtime: 'node' as const,
  memoryMb: 128,
  database: { schema: false, coreRefs: [] as string[] },
  routes: [
    { method: 'GET', path: '/delivery-log', policy: POLICY.deliveriesRead },
    { method: 'POST', path: '/test-delivery', policy: POLICY.deliveriesTest },
  ],
  permissions: [POLICY.deliveriesRead, POLICY.deliveriesTest] as string[],
  scopes: ['config:rw'] as const,
  ingestRoots: [] as string[],
};
