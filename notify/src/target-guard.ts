import * as dns from 'dns';
import { isInternalAddress } from './internal-address';

export type TargetCheck = { ok: true; url: URL } | { ok: false; reason: string };

/**
 * Re-resolved on every delivery attempt, not cached: a name that validated
 * earlier can repoint later (DNS rebinding), and the admin can edit the
 * target at any time. https-only; any resolved address that is internal
 * refuses the whole target, matching core's `PluginWebhookDispatcherService`.
 */
export async function checkTarget(rawUrl: string): Promise<TargetCheck> {
  let url: URL;
  try {
    url = new URL(rawUrl);
  } catch {
    return { ok: false, reason: 'not a valid URL' };
  }
  if (url.protocol !== 'https:') {
    return { ok: false, reason: 'target must be https:' };
  }

  let addresses: string[];
  try {
    addresses = (await dns.promises.lookup(url.hostname, { all: true })).map((a) => a.address);
  } catch (err) {
    return { ok: false, reason: `DNS lookup failed: ${(err as Error).message}` };
  }
  const internal = addresses.find(isInternalAddress);
  if (internal) {
    return { ok: false, reason: `"${url.hostname}" resolves to internal address ${internal}` };
  }
  return { ok: true, url };
}
