import * as net from 'net';

/**
 * Port of `backend/src/modules/plugins/internal-address.ts` from the Fliks
 * repo: same blocked ranges, same numeric-value judgement (not text shape),
 * because this plugin's admin-configured webhook target is exactly the
 * hazard that guard exists for.
 */
const BLOCKED_V4: ReadonlyArray<readonly [string, number]> = [
  ['0.0.0.0', 8],
  ['10.0.0.0', 8],
  ['100.64.0.0', 10],
  ['127.0.0.0', 8],
  ['169.254.0.0', 16],
  ['172.16.0.0', 12],
  ['192.0.0.0', 24],
  ['192.0.2.0', 24],
  ['192.168.0.0', 16],
  ['198.18.0.0', 15],
  ['198.51.100.0', 24],
  ['203.0.113.0', 24],
  ['224.0.0.0', 4],
  ['240.0.0.0', 4],
];

function v4ToInt(ip: string): number {
  return ip.split('.').reduce((n, octet) => n * 256 + Number(octet), 0) >>> 0;
}

function inV4Cidr(ip: string, base: string, bits: number): boolean {
  const mask = bits === 0 ? 0 : (0xffffffff << (32 - bits)) >>> 0;
  return ((v4ToInt(ip) & mask) >>> 0) === ((v4ToInt(base) & mask) >>> 0);
}

/** Eight 16-bit groups — fixed length by construction in {@link expandV6}, asserted once here
 *  rather than re-checked at every index below. */
type V6Groups = [number, number, number, number, number, number, number, number];

/** Expand any legal IPv6 text to its eight 16-bit groups. */
function expandV6(ip: string): V6Groups | null {
  const [head, tail] = ip.split('::');
  const parse = (s: string) => (s ? s.split(':').filter(Boolean).map((g) => parseInt(g, 16)) : []);
  const embedded = /(\d+\.\d+\.\d+\.\d+)$/.exec(ip)?.[1];
  const strip = (s: string) => (embedded ? s.replace(/(:)?[\d.]+$/, '') : s);
  const groups = embedded
    ? (() => {
        const n = v4ToInt(embedded);
        return [(n >>> 16) & 0xffff, n & 0xffff];
      })()
    : [];
  const left = parse(strip(head ?? ''));
  const right = tail === undefined ? [] : parse(strip(tail));
  const known = left.length + right.length + groups.length;
  if (tail === undefined) {
    return known === 8 ? ([...left, ...groups] as V6Groups) : null;
  }
  if (known > 8) return null;
  return [...left, ...new Array(8 - known).fill(0), ...right, ...groups] as V6Groups;
}

/**
 * True if `ip` is an address this plugin may not target: loopback,
 * link-local, private, CGNAT, reserved or multicast. Works on the numeric
 * value, not the text shape — `0:0:0:0:0:ffff:127.0.0.1` and `::ffff:127.0.0.1`
 * are the same address. Anything not a literal IP is refused, not guessed at.
 */
export function isInternalAddress(ip: string): boolean {
  const version = net.isIP(ip);
  if (version === 0) return true;

  if (version === 4) {
    return BLOCKED_V4.some(([base, bits]) => inV4Cidr(ip, base, bits));
  }

  const groups = expandV6(ip);
  if (!groups) return true;

  const isMapped = groups.slice(0, 5).every((g) => g === 0) && groups[5] === 0xffff;
  const isNat64 = groups[0] === 0x64 && groups[1] === 0xff9b && groups.slice(2, 6).every((g) => g === 0);
  if (isMapped || isNat64) {
    const n = (groups[6] << 16) + groups[7];
    const v4 = [24, 16, 8, 0].map((s) => (n >>> s) & 0xff).join('.');
    return BLOCKED_V4.some(([base, bits]) => inV4Cidr(v4, base, bits));
  }

  if (groups.every((g) => g === 0)) return true; // ::
  if (groups.slice(0, 7).every((g) => g === 0) && groups[7] === 1) return true; // ::1
  if ((groups[0] & 0xffc0) === 0xfe80) return true; // fe80::/10 link-local
  if ((groups[0] & 0xfe00) === 0xfc00) return true; // fc00::/7 unique-local
  if ((groups[0] & 0xff00) === 0xff00) return true; // ff00::/8 multicast
  return false;
}
