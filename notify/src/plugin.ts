import * as net from 'net';
import * as fs from 'fs';
import * as path from 'path';
import { attachDispatcher } from './dispatcher';
import { DeliveryQueue } from './delivery-queue';
import { handleHttp, type PluginHttpRequest } from './http-routes';
import { readConfig } from './config';
import { log } from './log';

const token = process.env.FLIKS_PLUGIN_TOKEN ?? '';
const pluginSockPath = process.env.FLIKS_PLUGIN_SOCK;
const coreSockPath = process.env.FLIKS_CORE_SOCK;

const config = readConfig();
const queue = new DeliveryQueue(() => config.targetUrl);

/** Read alongside `plugin.js` on every `hello` — the manifest core sees is
 *  always exactly the one that shipped in this archive, never a copy baked
 *  into the bundle that could drift from it. */
function loadManifest(): unknown {
  const raw = fs.readFileSync(path.join(__dirname, 'plugin.json'), 'utf8');
  return JSON.parse(raw);
}

const requestHandlers: Record<string, (payload: unknown) => Promise<unknown>> = {
  hello: async () => {
    return { manifest: loadManifest(), token };
  },

  health: async () => {
    const stats = queue.stats();
    return { ok: true, detail: `queued=${stats.queued} delivered=${stats.delivered} failed=${stats.failedPermanently}` };
  },

  job: async () => {
    return { ok: true };
  },

  http: async (payload: unknown) => {
    const p = payload as PluginHttpRequest;
    return handleHttp(queue, p);
  },

  shutdown: async () => {
    setTimeout(() => process.exit(0), 10); // let the reply flush before exiting
    return { ok: true };
  },
};

const noteHandlers: Record<string, (payload: unknown) => void> = {
  event: (payload) => {
    const p = payload as { name: string; payload: unknown };
    queue.enqueue(p.name, p.payload);
  },

  config: (payload) => {
    const p = payload as { changed: string[] };
    log.info(`config changed: ${p.changed.join(', ')} (takes effect on next restart)`);
  },
};

function connectInert(sockPath: string | undefined, label: string): void {
  if (!sockPath) return;
  const socket = net.connect(sockPath);
  socket.on('data', () => {}); // uplink is unused this phase — nothing calls the 17 core methods
  socket.on('error', (err) => log.warn(`${label} connection error: ${err.message}`));
}

function main(): void {
  if (!pluginSockPath) {
    log.error('FLIKS_PLUGIN_SOCK is not set; cannot start');
    process.exit(1);
  }

  connectInert(coreSockPath, 'core socket');

  const socket = net.connect(pluginSockPath);
  socket.on('connect', () => log.info(`connected to ${pluginSockPath}`));
  socket.on('error', (err) => log.error(`plugin socket error: ${err.message}`));
  attachDispatcher(socket, requestHandlers, noteHandlers);

  queue.start();
  log.info(`fliks.notify started (target=${config.targetUrl ?? 'unconfigured'})`);
}

main();
