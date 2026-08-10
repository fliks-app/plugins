import type { DeliveryQueue } from './delivery-queue';

export interface PluginHttpRequest {
  method: string;
  path: string;
  query: Record<string, string>;
  body: unknown;
}

export interface PluginHttpResponse {
  status: number;
  headers: Record<string, string>;
  body: unknown;
}

const JSON_HEADERS = { 'content-type': 'application/json' };

/** The plugin's own two routes — matched on `path` since core forwards the
 *  actual request path, already policy-checked against the declared route. */
export function handleHttp(queue: DeliveryQueue, req: PluginHttpRequest): PluginHttpResponse {
  if (req.method === 'GET' && req.path === '/delivery-log') {
    return { status: 200, headers: JSON_HEADERS, body: { entries: queue.recentLog(100), stats: queue.stats() } };
  }

  if (req.method === 'POST' && req.path === '/test-delivery') {
    const eventName = isRecord(req.body) && typeof req.body.event === 'string' ? req.body.event : 'test.delivery';
    const id = queue.enqueue(eventName, { triggeredBy: 'admin-test', at: new Date().toISOString() });
    return { status: 202, headers: JSON_HEADERS, body: { queued: true, id } };
  }

  return { status: 404, headers: JSON_HEADERS, body: { error: 'not found' } };
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}
