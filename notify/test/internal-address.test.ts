import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isInternalAddress } from '../src/internal-address';

/** Same adversarial table as `backend/src/modules/plugins/internal-address.spec.ts`
 *  in the Fliks repo — this is the boundary a plugin-configured webhook target
 *  must be checked against, so it has to agree with core on every case. */
const cases: [string, boolean][] = [
  ['127.0.0.1', true],
  ['10.1.2.3', true],
  ['169.254.169.254', true],
  ['172.16.0.1', true],
  ['172.31.255.255', true],
  ['192.168.1.1', true],
  ['0.0.0.0', true],
  ['::1', true],
  ['::', true],
  ['fe80::1', true],
  ['fc00::1', true],
  ['fd12:3456::1', true],
  ['::ffff:127.0.0.1', true],
  ['8.8.8.8', false],
  ['1.1.1.1', false],
  ['2606:4700::1111', false],
  ['172.32.0.1', false],
  ['172.15.0.1', false],
  ['192.169.1.1', false],
  // adversarial forms
  ['::ffff:10.0.0.1', true],
  ['::FFFF:169.254.169.254', true],
  ['0:0:0:0:0:ffff:127.0.0.1', true],
  ['64:ff9b::127.0.0.1', true],
  ['fe80:0000:0000:0000:0000:0000:0000:0001', true],
  ['100.64.0.1', true],
  ['198.18.0.1', true],
  ['192.0.0.1', true],
  ['255.255.255.255', true],
  ['224.0.0.1', true],
];

for (const [ip, blocked] of cases) {
  test(`isInternalAddress(${ip}) === ${blocked}`, () => {
    assert.equal(isInternalAddress(ip), blocked);
  });
}

test('refuses anything that is not a literal IP', () => {
  for (const bogus of ['', 'example.com', '127.0.0.1.evil.com', '0x7f000001']) {
    assert.equal(isInternalAddress(bogus), true);
  }
});
