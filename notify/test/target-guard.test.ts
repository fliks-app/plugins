import { test } from 'node:test';
import assert from 'node:assert/strict';
import { checkTarget } from '../src/target-guard';

test('refuses a non-https target without doing a DNS lookup', async () => {
  const result = await checkTarget('http://example.com/hook');
  assert.equal(result.ok, false);
  if (!result.ok) assert.match(result.reason, /https/);
});

test('refuses an unparsable target', async () => {
  const result = await checkTarget('not a url');
  assert.equal(result.ok, false);
});

test('refuses an https target that is a literal internal address (no network needed)', async () => {
  const result = await checkTarget('https://127.0.0.1/hook');
  assert.equal(result.ok, false);
  if (!result.ok) assert.match(result.reason, /internal address/);
});

test('accepts an https target that is a literal public address (no network needed)', async () => {
  const result = await checkTarget('https://8.8.8.8/hook');
  assert.equal(result.ok, true);
});
