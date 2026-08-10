import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { build } from '../scripts/build';

const ROOT = path.join(__dirname, '..');
const DIST = path.join(ROOT, 'dist');

/** Minimal comparator-range check for the exact ">=X.Y.Z <A.B.C" shape this
 *  build ever emits — not a general semver parser, so no `semver` dependency
 *  just to check one string in a test. */
function tupleOf(v: string): [number, number, number] {
  const [maj, min, pat] = v.split('.').map(Number);
  return [maj ?? 0, min ?? 0, pat ?? 0];
}
function cmp(a: [number, number, number], b: [number, number, number]): number {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i]! - b[i]!;
  return 0;
}
function satisfiesRange(version: string, range: string): boolean {
  const v = tupleOf(version);
  return range
    .trim()
    .split(/\s+/)
    .every((clause) => {
      const m = /^(>=|<)(\d+\.\d+\.\d+)$/.exec(clause);
      if (!m) throw new Error(`unsupported clause: ${clause}`);
      const bound = tupleOf(m[2]!);
      return m[1] === '>=' ? cmp(v, bound) >= 0 : cmp(v, bound) < 0;
    });
}

/** Prefer the sibling Fliks checkout's real version when present; otherwise
 *  fall back to the value read from it at authoring time. */
function currentFliksVersion(): string {
  const siblingPkg = path.join(ROOT, '..', 'fliks', 'backend', 'package.json');
  if (fs.existsSync(siblingPkg)) {
    const pkg = JSON.parse(fs.readFileSync(siblingPkg, 'utf8')) as { version: string };
    return pkg.version;
  }
  return '2.0.1';
}

test('build emits a manifest whose files hashes match the real bundle bytes', () => {
  build();
  const manifest = JSON.parse(fs.readFileSync(path.join(DIST, 'plugin.json'), 'utf8')) as {
    files: Record<string, string>;
  };

  for (const [name, expectedHash] of Object.entries(manifest.files)) {
    const actual = createHash('sha256').update(fs.readFileSync(path.join(DIST, name))).digest('hex');
    assert.equal(actual, expectedHash, `sha256 of ${name} must match manifest.files`);
  }
  assert.deepEqual(Object.keys(manifest.files).sort(), ['logo.svg', 'plugin.js']);
});

test('the fliks range satisfies the real Fliks version', () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(DIST, 'plugin.json'), 'utf8')) as { fliks: string };
  const version = currentFliksVersion();
  assert.ok(satisfiesRange(version, manifest.fliks), `${manifest.fliks} must satisfy ${version}`);
  // and the range must carry a mandatory upper bound, not just a floor
  assert.match(manifest.fliks, /<\d+\.\d+\.\d+/);
});

test('manifest carries the required process-tier shape', () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(DIST, 'plugin.json'), 'utf8')) as Record<string, unknown>;
  assert.equal(manifest.id, 'fliks.notify');
  assert.equal(manifest.kind, 'process');
  assert.equal(manifest.pluginApi, 0);
  assert.equal(manifest.runtime, 'node');
  assert.deepEqual(manifest.database, { schema: false, coreRefs: [] });
  assert.deepEqual(manifest.scopes, ['config:rw']);
  assert.deepEqual(manifest.ingestRoots, []);
  const routes = manifest.routes as { method: string; path: string; policy: string }[];
  assert.ok(routes.length >= 2);
  for (const route of routes) assert.ok(route.policy && route.policy.length > 0);

  // routes point at the plugin's own declared permissions, not a core CASL subject
  const permissions = manifest.permissions as string[];
  assert.equal(permissions.length, 2);
  for (const route of routes) assert.ok(permissions.includes(route.policy));
  for (const permission of permissions) assert.match(permission, /^fliks\.notify:/);
});
