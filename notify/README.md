# Notify

Sends a message to a service of yours every time something happens in Fliks — a film finishes
importing, a request is approved. You give it one HTTPS address; it posts the event there as JSON.

Use it to drive whatever you already have: a chat channel through a webhook relay, a home
automation flow, a script that keeps its own log.

## What you get over the built-in webhook plugin

Fliks also ships a simpler webhook plugin that posts events directly. Pick this one when delivery
matters more than simplicity:

- **It retries.** A target that is down or slow gets six attempts over about two minutes instead
  of one shot.
- **It absorbs a burst.** A library scan that fires hundreds of events queues them instead of
  hammering your endpoint, and drops the oldest rather than growing without limit.
- **It shows you what happened.** Recent attempts and their outcomes are readable through the
  plugin's API (`GET /delivery-log`), so a delivery that failed is not a mystery.
- **It can be tested.** `POST /test-delivery` sends a synthetic event, so you can confirm your
  endpoint works before waiting for something real. Neither has a button in the settings UI yet.

If none of that matters to you, the built-in plugin is less to run.

## Setting it up

Install it on Fliks 5, set **the target URL** in its settings page, and use the test delivery to
confirm your endpoint answers. The URL often carries a token, so it is stored as a secret and never
written to the logs.

Two rules the plugin will not bend on:

- **HTTPS only.** A plain-http target is refused.
- **Public addresses only.** A target on your own machine or your private network is refused —
  and re-checked on every single attempt, not just when you save it, because a name can be
  repointed after the fact. The Fliks server does the check when it connects.

Changing the target takes effect on the next attempt, queued deliveries included.

## What it does not do

- **No filtering.** Every event Fliks forwards is delivered; there is no per-event switch.
- **No exactly-once guarantee.** Queued deliveries and their retry timers survive restarts and
  upgrades, but delivery is at least once: after a crash a delivery may be sent twice. The `id` in
  the body is stable, so a receiver can deduplicate. After six failed attempts a delivery is dropped
  and recorded.

## Contributing

Bounds, the delivery queue, the build and the test suite are in
[docs/development.md](docs/development.md).
