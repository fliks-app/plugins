import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DeliveryQueue, QUEUE_CAPACITY, LOG_CAPACITY, MAX_ATTEMPTS, BACKOFF_MS } from '../src/delivery-queue';

test('drops the oldest queued item once capacity is reached', () => {
  const queue = new DeliveryQueue(() => 'https://example.invalid/hook');
  for (let i = 0; i < QUEUE_CAPACITY + 10; i++) queue.enqueue('media.imported', { i });

  assert.equal(queue.size, QUEUE_CAPACITY);
  assert.equal(queue.stats().droppedForCapacity, 10);
  assert.ok(queue.recentLog(LOG_CAPACITY + 50).length <= LOG_CAPACITY);
});

test('an unconfigured target never enters the queue but is still logged', () => {
  const queue = new DeliveryQueue(() => null);
  const id = queue.enqueue('media.imported', { a: 1 });
  assert.equal(queue.size, 0);
  const entry = queue.recentLog(10).find((e) => e.id === id);
  assert.equal(entry?.lastStatus, 'refused:no target configured');
});

test('a non-https target is refused on the first attempt, not retried', async (t) => {
  t.mock.timers.enable({ apis: ['Date'] });
  const queue = new DeliveryQueue(() => 'http://example.invalid/hook');
  const id = queue.enqueue('media.imported', { a: 1 });
  await queue.runOnce();

  assert.equal(queue.size, 0); // no retry scheduled
  const entry = queue.recentLog(10).find((e) => e.id === id);
  assert.match(entry?.lastStatus ?? '', /^refused:/);
  assert.equal(entry?.attempt, 1);
});

test('retries a failing target on the backoff ladder and drops after MAX_ATTEMPTS', async (t) => {
  t.mock.timers.enable({ apis: ['Date'] });
  let calls = 0;
  const queue = new DeliveryQueue(
    () => 'https://8.8.8.8/hook',
    async () => {
      calls++;
      throw new Error('target unreachable');
    },
  );
  const id = queue.enqueue('media.imported', { a: 1 });

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    await queue.runOnce(); // due immediately: first try, or fast-forwarded past backoff below
    const entry = queue.recentLog(10).find((e) => e.id === id);
    assert.equal(entry?.attempt, attempt);
    if (attempt < MAX_ATTEMPTS) {
      assert.equal(queue.size, 1, `item stays queued after attempt ${attempt}`);
      assert.match(entry?.lastStatus ?? '', /^retrying:/);
      t.mock.timers.tick(BACKOFF_MS[attempt - 1]! + 1);
    } else {
      assert.equal(queue.size, 0, 'dropped after the final attempt');
      assert.match(entry?.lastStatus ?? '', /^failed:/);
    }
  }

  assert.equal(calls, MAX_ATTEMPTS);
  assert.equal(queue.stats().failedPermanently, 1);
});

test('a delivery that succeeds is logged as delivered and not retried', async (t) => {
  t.mock.timers.enable({ apis: ['Date'] });
  const queue = new DeliveryQueue(
    () => 'https://8.8.8.8/hook',
    async () => 200,
  );
  const id = queue.enqueue('media.imported', { a: 1 });
  await queue.runOnce();

  assert.equal(queue.size, 0);
  const entry = queue.recentLog(10).find((e) => e.id === id);
  assert.equal(entry?.lastStatus, 'delivered:200');
  assert.equal(queue.stats().delivered, 1);
});
