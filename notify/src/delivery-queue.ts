import * as https from 'https';
import { checkTarget } from './target-guard';
import { log } from './log';

export interface DeliveryLogEntry {
  id: number;
  event: string;
  targetHost: string;
  attempt: number;
  lastStatus: string;
  createdAt: string;
  lastAttemptAt: string | null;
}

interface QueueItem {
  id: number;
  event: string;
  payload: unknown;
  targetUrl: string;
  attempt: number;
  nextAttemptAt: number;
}

/** Bounds: a burst (e.g. one library scan) can queue up to 200 pending
 *  deliveries before the oldest is dropped rather than growing unbounded.
 *  Each item retries up to 6 times on a ladder topping out at 2 minutes
 *  (~4 minutes of total retrying) before it is dropped and logged. */
export const QUEUE_CAPACITY = 200;
export const LOG_CAPACITY = 200;
export const MAX_ATTEMPTS = 6;
export const BACKOFF_MS = [1_000, 5_000, 15_000, 30_000, 60_000, 120_000];
const REQUEST_TIMEOUT_MS = 5_000;
const TICK_MS = 1_000;

export class DeliveryQueue {
  private queue: QueueItem[] = [];
  private logEntries: DeliveryLogEntry[] = [];
  private logById = new Map<number, DeliveryLogEntry>();
  private nextId = 1;
  private timer: NodeJS.Timeout | null = null;
  private inFlight = false;
  private droppedForCapacity = 0;
  private delivered = 0;
  private failedPermanently = 0;

  /** `deliver` is injectable so tests can exercise the retry ladder and the
   *  capacity bound without a real network call; defaults to a real POST. */
  constructor(
    private readonly getTargetUrl: () => string | null,
    private readonly deliver: (url: URL, body: unknown) => Promise<number> = postJson,
  ) {}

  start(): void {
    if (this.timer) return;
    this.timer = setInterval(() => void this.tick(), TICK_MS);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  get size(): number {
    return this.queue.length;
  }

  /** Drives one processing cycle synchronously — the timer calls the same
   *  method; exposed so tests don't have to wait on the real interval. */
  async runOnce(): Promise<void> {
    await this.tick();
  }

  stats(): { queued: number; delivered: number; failedPermanently: number; droppedForCapacity: number } {
    return {
      queued: this.queue.length,
      delivered: this.delivered,
      failedPermanently: this.failedPermanently,
      droppedForCapacity: this.droppedForCapacity,
    };
  }

  recentLog(limit = 100): DeliveryLogEntry[] {
    return this.logEntries.slice(-limit).reverse();
  }

  /** Enqueue one delivery. Drops the oldest queued item rather than growing without limit. */
  enqueue(event: string, payload: unknown): number {
    const targetUrl = this.getTargetUrl();
    const id = this.nextId++;
    const now = Date.now();
    const entry: DeliveryLogEntry = {
      id,
      event,
      targetHost: hostOf(targetUrl),
      attempt: 0,
      lastStatus: targetUrl ? 'pending' : 'refused:no target configured',
      createdAt: new Date(now).toISOString(),
      lastAttemptAt: null,
    };
    this.pushLog(entry);

    if (!targetUrl) {
      log.warn(`event "${event}" dropped: no delivery target configured`);
      return id;
    }

    if (this.queue.length >= QUEUE_CAPACITY) {
      const dropped = this.queue.shift();
      this.droppedForCapacity++;
      if (dropped) this.markLog(dropped.id, 'dropped:queue full', dropped.attempt);
      log.warn(`delivery queue at capacity (${QUEUE_CAPACITY}); dropped oldest item`);
    }
    this.queue.push({ id, event, payload, targetUrl, attempt: 0, nextAttemptAt: now });
    return id;
  }

  private pushLog(entry: DeliveryLogEntry): void {
    if (this.logEntries.length >= LOG_CAPACITY) this.logEntries.shift();
    this.logEntries.push(entry);
    this.logById.set(entry.id, entry);
  }

  private markLog(id: number, status: string, attempt: number): void {
    const entry = this.logById.get(id);
    if (!entry) return;
    entry.lastStatus = status;
    entry.attempt = attempt;
    entry.lastAttemptAt = new Date().toISOString();
  }

  private async tick(): Promise<void> {
    if (this.inFlight) return; // one delivery in flight at a time — a slow target must not pile up sockets
    const now = Date.now();
    const idx = this.queue.findIndex((item) => item.nextAttemptAt <= now);
    if (idx === -1) return;
    const [item] = this.queue.splice(idx, 1);
    if (!item) return;
    this.inFlight = true;
    try {
      await this.attempt(item);
    } finally {
      this.inFlight = false;
    }
  }

  private async attempt(item: QueueItem): Promise<void> {
    item.attempt++;
    const check = await checkTarget(item.targetUrl);
    if (!check.ok) {
      this.markLog(item.id, `refused:${check.reason}`, item.attempt);
      log.warn(`delivery ${item.id} refused: ${check.reason}`);
      return; // a refused target is not transient — no retry
    }

    try {
      const status = await this.deliver(check.url, {
        id: item.id,
        event: item.event,
        payload: item.payload,
        attempt: item.attempt,
      });
      this.markLog(item.id, `delivered:${status}`, item.attempt);
      this.delivered++;
    } catch (err) {
      const reason = (err as Error).message;
      if (item.attempt >= MAX_ATTEMPTS) {
        this.markLog(item.id, `failed:${reason}`, item.attempt);
        this.failedPermanently++;
        log.warn(`delivery ${item.id} dropped after ${item.attempt} attempts: ${reason}`);
        return;
      }
      const delay = BACKOFF_MS[Math.min(item.attempt - 1, BACKOFF_MS.length - 1)]!;
      item.nextAttemptAt = Date.now() + delay;
      this.markLog(item.id, `retrying:${reason}`, item.attempt);
      this.queue.push(item);
    }
  }
}

function hostOf(url: string | null): string {
  if (!url) return 'unconfigured';
  try {
    return new URL(url).hostname;
  } catch {
    return 'invalid';
  }
}

function postJson(url: URL, body: unknown): Promise<number> {
  const data = Buffer.from(JSON.stringify(body), 'utf8');
  return new Promise((resolve, reject) => {
    const req = https.request(
      url,
      {
        method: 'POST',
        timeout: REQUEST_TIMEOUT_MS,
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': data.length,
          'User-Agent': 'fliks.notify/0.1.0',
        },
      },
      (res) => {
        res.resume(); // drain and discard — this plugin never reads the response body
        const status = res.statusCode ?? 0;
        if (status >= 200 && status < 300) resolve(status);
        else reject(new Error(`target replied ${status}`));
      },
    );
    req.on('timeout', () => req.destroy(new Error(`timed out after ${REQUEST_TIMEOUT_MS}ms`)));
    req.on('error', (err) => reject(err));
    req.end(data);
  });
}
