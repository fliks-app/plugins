# Rust port (pluginApi 2): fk-plugin-notify

Spec for porting `fliks.notify` from Node/TypeScript to a Rust WebAssembly component (pluginApi 2,
`runtime: "wasm"`, target `wasm32-wasip2`). Phase 2, milestone S8. Docs only: no code, no build,
nothing else in this repository changes with this file.

Sources read in full at `3ec27cf`: every file under `src/`, `scripts/`, `test/`, `README.md`,
`docs/development.md`, CI. Tags: `[ts:path:line]` is a line of this repository at that commit, `[new]`
a decision of this port, `[assumed]` a guess to confirm in the S8 spike. The host side is
`server/docs/spec/plugins.md` (merged), cited as `plugins.md rule N`. The guest helpers this port
needs are listed once, in section 6 of the sibling spec `fk-plugin-download/docs/rust-port.md`
("the SDK list", items `SDK-1` to `SDK-13`); section 6 below only says which ones apply.

Contents: 0 findings, 1 inventory, 2 mapping onto the WIT world, 3 state and where it moves,
4 database, 5 crate layout, 6 SDK, 7 test plan, 8 packaging and catalog, 9 deviations,
10 decisions.

## 0. Findings that shape the port

1. **There is no pluginApi 1 version to upgrade from.** The manifest template still says
   `pluginApi: 0` and `fliks: '>=2.0.0 <3.0.0'` [ts:scripts/manifest-template.ts:41,43], and a test pins
   `pluginApi === 0` [ts:test/manifest.test.ts:71]. The catalog's `COMPATIBILITY.md` retired `0` with
   core 4.0.0. The plugin is also absent from `fliks-plugin-catalog` (only `fliks.download` and
   `fliks.webhooks` are there). So no Fliks 4 server can have it installed from the catalog, nothing
   was ever stored, and the port is a first release for Fliks 5. "Keep existing user data" reduces to
   one fact: a leftover `plugin.fliks.notify.*` setting keeps working (section 4).
2. **The manifest has no UI.** README says "set the target URL in its settings page" and "a button
   sends a synthetic event" [README.md:20-28]; the template declares no `ui`, no `configPages`, no
   field for the URL [ts:scripts/manifest-template.ts:39-59]. The URL arrives as the env var
   `FLIKS_CFG_TARGET_URL` [ts:src/config.ts:13-15], i.e. a setting `target_url`, `target-url` or
   `target.url` that an admin could only create by writing the setting by hand [assumed: `target_url`].
   The port adds the settings page (NT6).
3. **The retry ladder has a dead step.** `BACKOFF_MS` has six entries but `MAX_ATTEMPTS = 6` drops the
   item after the sixth attempt, so only five delays are ever used: 1, 5, 15, 30, 60 s, about
   111 s in total [ts:src/delivery-queue.ts:30-31,168-175]. The comment and the README say "about four
   minutes" [ts:src/delivery-queue.ts:24-27, README.md:14-15]. The port keeps the behaviour (6 attempts,
   5 delays) and fixes the wording (README fixed in the same PR; D8).
4. **A DNS failure is a refusal, not a retry.** `checkTarget` returns `ok:false` on a failed lookup and
   `attempt` returns without scheduling a retry [ts:src/target-guard.ts:23-28,
   ts:src/delivery-queue.ts:151-155]. Kept for parity (D8).
5. **Everything in memory today is correctness state** (the queue, its attempt counters, the log, the
   counters). It all moves; section 3.

## 1. Inventory of behaviour

### 1.1 Manifest today [ts:scripts/manifest-template.ts:25-59]

| Field | Now | In the port |
|---|---|---|
| `id` | `fliks.notify` | same |
| `pluginApi`, `fliks`, `runtime` | `0`, `>=2.0.0 <3.0.0`, `node` | `2`, `>=5.0.0 <6.0.0`, `wasm` |
| `kind`, `memoryMb` | `process`, `128` | `process`, `64` [assumed; spike criterion: idle instance under 40 MiB resident] |
| `database` | `{schema: false, coreRefs: []}` | `{schema: true, coreRefs: []}` (section 4) |
| `scopes` | `['config:rw']` | same (`config.get` only) |
| `ingestRoots`, `jobs`, `network` | none | none; no `network.internal`: public targets only |
| `permissions` | `delivery-log`, `test-delivery` | same |
| `routes` | 2 | same, section 1.2 |
| `ui`, `i18n` | none | one form page, section 1.6 |

### 1.2 Routes (2) [ts:src/http-routes.ts:18-32]

| Route | Policy | Behaviour |
|---|---|---|
| `GET /delivery-log` | `read:plugin:fliks.notify:delivery-log` | 200 `{entries: [<= 100 newest first], stats: {queued, delivered, failedPermanently, droppedForCapacity}}`; an entry is `{id, event, targetHost, attempt, lastStatus, createdAt, lastAttemptAt}` [ts:src/delivery-queue.ts:5-13,75-86] |
| `POST /test-delivery` | `create:plugin:fliks.notify:test-delivery` | event name is `body.event` when a string, else `test.delivery`; payload `{triggeredBy: 'admin-test', at: <ISO>}`; answers 202 `{queued: true, id}` even when the delivery is refused at once for want of a target |
| anything else | n/a | 404 `{error: 'not found'}`; `content-type: application/json` on every answer |

No literal collision with the core routes of C106.

### 1.3 Events consumed

Every domain event, unfiltered ("no filtering", README.md:41), as `{name, payload}` notes
[ts:src/plugin.ts:50-54]. The manifest has no `events` (that key is the data tier's webhook list; a
process plugin gets all events, `plugins.md` rule 46).

### 1.4 Delivery behaviour [ts:src/delivery-queue.ts:24-216]

| Rule | Value |
|---|---|
| Enqueue with no target configured | log entry `refused:no target configured`, warn, nothing queued |
| Pending bound | 200; the 201st drops the oldest pending (`dropped:queue full`, counter, warn) |
| Log bound | 200 entries, oldest evicted, `GET` returns the newest 100 |
| In flight | exactly one delivery at a time, 1 s tick, first due item in queue order |
| Target check, every attempt | parse, `https:` only (`refused:target must be https:`), resolve, any internal address refuses (`refused:"<host>" resolves to internal address <ip>`); refusals and DNS failures are final |
| Request | `POST <target>`, `Content-Type: application/json`, `User-Agent: fliks.notify/0.1.0`, body `{id, event, payload, attempt}`, 5 s idle timeout, response body drained and discarded |
| Success | any 2xx: `delivered:<status>` |
| Failure | non-2xx (`target replied N`), timeout, socket error: `retrying:<reason>` after attempts 1-5 with delays 1, 5, 15, 30, 60 s; after attempt 6 `failed:<reason>`, counter, warn |
| `id` | in-memory counter from 1, restarting at every process start |

### 1.5 Health, shutdown, other exports

`health` answers `{ok: true, detail: 'queued=Q delivered=D failed=F'}` [ts:src/plugin.ts:30-33];
`job` answers `{ok: true}` for any name though no job is declared :35-37; `config` notes are
logged with "takes effect on next restart" :56-59; `shutdown` exits after the reply :44-47. The plugin
opens a second socket to core and never uses it [ts:src/plugin.ts:62-67].

### 1.6 UI, config, outbound calls

- UI today: none (finding 2). Config today: env `FLIKS_CFG_TARGET_URL`, fixed for the process life,
  trimmed, empty means unconfigured.
- Outbound: one HTTPS POST per attempt to the admin's URL. No webhook, no catalog call, no host
  call other than the (unused) uplink socket.
- Port: `ui.contributions` one `settings.page` entry (`fliks-notify.settings.general`, weight 100,
  action route `/admin/settings/plugins/fliks.notify/general`) and `ui.configPages` one form page
  `general` with a single field `{key: 'target_url', type: 'url', labelKey, hint, secret: true}`
  [webhook URLs often embed a token; decided, D8]; `i18n` `en` and `fr`, keys
  `notify.config.title`, `.subtitle`, `.target_url`, `.target_url_hint` (one root branch `notify`,
  no clash with `download`). Format as in `fk-plugin-download` [scripts/manifest-template.ts there].
  The delivery log and the test button stay API-only: the log route returns `{entries, stats}`,
  not the `{data, total, page, pageSize}` a `table` page reads, and a form page has no action
  button. Showing them needs a UI contract change; out of scope, proposed issue in section 10.

## 2. Mapping onto the WIT world `fliks:plugin@2.0.0`

| Export | Notify behaviour |
|---|---|
| `hello(core-version, config)` | SDK answers `manifest-check`. Config is not stored: the URL is read per use. |
| `start()` | `db.migrate([0001_deliveries])`, then the stuck-row sweep of 3.3. |
| `health()` | one `SELECT` of the counters and the pending count: `ok:true`, detail `queued=Q delivered=D failed=F`; `ok:false` with the error text if the database does not answer (3 s deadline). |
| `job(...)` | no job declared; `Err("no jobs")` (Node answered ok to any name). |
| `event(name, payload)` | `enqueue(name, payload)` of 3.2 (event lane, arrival order, 60 s deadline; each event costs about four short calls). |
| `scheduled(key, payload, attempt)` | key `delivery:<id>`: one attempt of 3.2. An unknown prefix is dropped with a warn and `Ok`. |
| `http(request)` | the two routes of 1.2, from the SDK router. |
| `config-changed(keys)` | no-op (the URL is read at every attempt); log at info. |
| `shutdown()` | no-op; nothing lives in the instance. |

Lanes (`plugins.md` rule 40): `core` x1, `event` x1, `http` 0 to 4 (the two routes are a `SELECT`
and an `INSERT`), `task` 0 to 2 (deliveries), `job` none. No other lane ever blocks `event`: a slow
target holds a `task` instance, never the event lane or `health`.

## 3. State and where it moves

### 3.1 Inventory

| State | Where today | Destination |
|---|---|---|
| `queue[]` with `attempt`, `nextAttemptAt`, `payload` | `delivery-queue.ts:36,15-22` | one scheduled call per delivery, key `delivery:<id>` (3.2) |
| `inFlight`, 1 s timer | `:40-41,53-56,133-146` | gone: the host claims due calls; up to 2 run at once (NT4) |
| `logEntries`, `logById`, `nextId` | `:37-39,119-131` | table `deliveries` (4) |
| `delivered`, `failedPermanently`, `droppedForCapacity` | `:42-44` | table `delivery_stats` (4) |
| `config.targetUrl` | `config.ts`, `plugin.ts:14` | `config.get(['target_url'])` at each use (NT2) |
| Core socket and framing | `dispatcher.ts`, `protocol.ts`, `plugin.ts:62-84` | gone: WIT exports |

No instance holds anything a later call needs.

### 3.2 `delivery-queue.ts` into scheduled calls (one key per delivery)

`enqueue(event, payload)` (event lane, and the `test-delivery` route):

1. `target = config.get('target_url')` (trimmed; empty is none).
2. `INSERT INTO deliveries (...) RETURNING id`. State `pending`, or `refused` with
   `lastStatus = 'refused:no target configured'` when there is no target. `targetHost` is the URL's
   hostname, `unconfigured` or `invalid` as `hostOf` does [ts:src/delivery-queue.ts:182-189].
3. No target: warn and stop.
4. Capacity: one statement marks the oldest live rows beyond 200 `dropped:queue full` and bumps
   `droppedForCapacity` by the same count (CTE: `SELECT id ... WHERE state IN ('pending','retrying')
   ORDER BY id DESC OFFSET 200 FOR UPDATE SKIP LOCKED`, the `UPDATE ... RETURNING id`, the stats
   update `FROM` it), then `cancel-call("delivery:<id>")` per returned id, warn. Two enqueuers never
   drop or count one row twice. The bound is soft by at most the number of concurrent `http` instances
   (the route and the event lane can enqueue at the same time).
5. `schedule-call("delivery:<id>", 0, {"event": ..., "payload": ..., "n": 0})`. A payload over
   64 KiB (`ERR_VALIDATION`) marks the row `dropped:<the host's message>` [new, NT7].
6. Housekeeping on the same call: prune terminal rows beyond the newest 200
   (`DELETE FROM deliveries WHERE state NOT IN ('pending','retrying','lost') AND id <= (SELECT id FROM
   deliveries ORDER BY id DESC OFFSET 200 LIMIT 1)`).

`scheduled("delivery:<id>", payload, host_attempt)` (task lane, 5 min deadline [assumed]; one attempt is
at most connect + first byte + between bytes at 5 s each):

0. Read the row; missing or terminal (`delivered`, `failed`, `refused`, `dropped`): `Ok`, no POST
   (a capacity drop or a re-fire after the outcome was written).
1. Read `{event, payload, n}` ignoring unknown fields; `n` counts completed attempts. The host's
   `attempt` counts claims, including re-fires after a trap, so it is only logged.
2. `target = config.get('target_url')`; none: `refused:no target configured`, final.
3. Parse the URL (`url` crate): invalid `refused:not a valid URL`, scheme other than `https`
   `refused:target must be https:`, final.
4. `n += 1`; POST through the SDK client with JSON body `{id, event, payload, attempt: n}`, headers as
   in 1.4, `RequestOptions` connect, first-byte and between-bytes 5 s, redirects not followed, response
   body dropped unread.
5. Outcome:

| Result | Row | Next |
|---|---|---|
| 2xx | `delivered`, `lastStatus = delivered:<status>`, `delivered` counter | none |
| host guard denial (`HttpRequestDenied`) | `refused`, `refused:<reason>` [assumed text: `target address is not allowed`] | none |
| DNS error | `refused`, `refused:DNS lookup failed: <detail>` (parity, finding 4) | none |
| anything else (non-2xx, timeout, refused connection, TLS, reset) and `n < 6` | `retrying`, `retrying:<reason>`, `attempt = n` | `schedule-call("delivery:<id>", BACKOFF_MS[n-1], {event, payload, n})` replaces the claimed row (that is how `Ok` after a re-schedule keeps it, `plugins.md` rule 55) |
| the same and `n = 6` | `failed`, `failed:<reason>`, `failedPermanently` counter, warn | none |

Every row write of step 5 is conditional (`WHERE id = $1 AND state IN ('pending','retrying','lost')`)
and the retry `schedule-call` runs only when it updated a row, so a row dropped for capacity while
its call ran is neither overwritten nor re-scheduled.

6. Return `Ok` after the row and the schedule are written. `Err` is reserved for "could not write
   state": the host re-fires after its lease (6 min), which at worst repeats the POST. Delivery is
   at-least-once; the stable `id` in the body lets a receiver deduplicate.

`BACKOFF_MS = [1000, 5000, 15000, 30000, 60000]` in the port; the sixth value of the TS table is
unreachable (finding 3).

The queue order is now best effort (NT4), the retry timer survives a host restart and an upgrade
(pending calls live in `plugin_scheduled_calls`, `plugins.md` rule 55), and a disabled plugin keeps its
pending calls until it is enabled again.

### 3.3 Stuck rows

A row stays `pending` or `retrying` only while a call is owed. If the host drops a poison call (20
attempts, rule 55) or the event lane inserted a row and the host stopped before `schedule-call`, the row
would read pending forever. At `start` and at every enqueue:
`UPDATE deliveries SET state='lost', "lastStatus"='failed:lost' WHERE state IN ('pending','retrying')
AND "updatedAt" < now() - interval '3 hours'` [assumed threshold: past the host's 20 re-fires 6 min
apart]. `lost` is display only: it leaves `queued` and the capacity count, is never pruned, and a call
that still fires (after a long disable, a backlog) delivers it normally (step 0 accepts it).

## 4. Database

Schema `plugin_fliks_notify`, provisioned by the host on install (`database.schema: true`, no
`coreRefs`), dropped with the plugin. One migration, `0001_deliveries`:

```sql
CREATE TABLE "deliveries" (
  "id"            bigserial PRIMARY KEY,
  "event"         text NOT NULL,
  "targetHost"    text NOT NULL,
  "state"         text NOT NULL,      -- pending | retrying | lost | delivered | failed | refused | dropped
  "attempt"       integer NOT NULL DEFAULT 0,
  "lastStatus"    text NOT NULL,
  "createdAt"     timestamptz NOT NULL DEFAULT now(),
  "lastAttemptAt" timestamptz,
  "updatedAt"     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX "idx_deliveries_state_id" ON "deliveries" ("state", "id");

CREATE TABLE "delivery_stats" (
  "id"                 smallint PRIMARY KEY CHECK ("id" = 1),
  "delivered"          bigint NOT NULL DEFAULT 0,
  "failedPermanently"  bigint NOT NULL DEFAULT 0,
  "droppedForCapacity" bigint NOT NULL DEFAULT 0
);
INSERT INTO "delivery_stats" ("id") VALUES (1);
```

The payload is not stored: it lives in the scheduled call, the row keeps only what the log shows.
Terminal transitions update the row and the counter in one
statement (`WITH u AS (UPDATE "deliveries" ... WHERE ... state IN (...) RETURNING 1) UPDATE
"delivery_stats" SET ... WHERE EXISTS (SELECT 1 FROM u)`) so they cannot disagree: the counter moves
only when the conditional row update did. Result types: `int8`, `int4`, `text`, `timestamptz`
only (`plugins.md` rule 35).
`GET /delivery-log` is `SELECT ... FROM deliveries ORDER BY id DESC LIMIT 100` plus the stats row and
`SELECT count(*) ... WHERE state IN ('pending','retrying')` for `queued`.

User data across the upgrade: nothing to migrate (finding 1). The setting `plugin.fliks.notify.target_url`,
if an admin created it, is read by the new code unchanged; any other key naming the URL is ignored
until re-entered on the new page. Settings ownership (F68): `fliks.notify` is not a prefix of another
installed id and no other id is a prefix of it.

## 5. Crate layout and dependencies

```
fk-plugin-notify/
  Cargo.toml              workspace: plugin crate + xtask (SDK-12)
  rust-toolchain.toml     same pin as fk-plugin-download
  plugin.template.json    manifest without version/files
  logo.svg
  migrations/0001_deliveries.sql
  src/lib.rs              export! and the Guest impl
  src/queue.rs            enqueue, eviction, pruning, stuck-row sweep
  src/delivery.rs         one attempt, outcome classification, BACKOFF_MS
  src/log_db.rs           the statements of section 4
  src/routes.rs           the two routes and their JSON
  tests/                  vectors, component tests
```

Dependencies: `fliks-plugin-sdk` (git tag), `serde`, `serde_json`, `url`. Nothing else: no `regex`, no
time crate (the SDK gives RFC 3339), no async runtime (one blocking request per call, `SDK-5` not
needed), no TLS, no DNS, no address-class code. `plugin.wasm` target under 2 MiB [assumed].

Deleted with the port: `src/{plugin,dispatcher,protocol,config,log,delivery-queue,target-guard,
internal-address,http-routes}.ts`, `scripts/*`, `package.json`, `tsconfig.json`, the Node CI.

## 6. SDK items used (numbers from the download spec, section 6)

Needed: SDK-1 (`export!`, `manifest-check`), SDK-2 (only `config-get`), SDK-3 (schedule/cancel,
prefix dispatch, tolerant payloads), SDK-4 (`db`, `migrate`), SDK-6 (RFC 3339 now, for the test payload `at`), SDK-7 (one POST with the three timeouts, error taxonomy
including `Denied` and `Dns`), SDK-8 (router, two routes), SDK-9 (`config::get`), SDK-10 (logging),
SDK-11 (testing: component harness with a controllable clock and a programmable target), SDK-12
(build and package), SDK-13 (licence `MIT OR Apache-2.0`, D5; notify stays MIT).
Not needed: SDK-5 (executor and budgets beyond a plain deadline constant), so SDK-7 must offer a
blocking send (block on the response pollable) that works without the SDK-5 executor.

Two behaviours the SDK must fix in writing because the notify tests depend on them: the HTTP client
maps a host guard denial to its own error variant (not a generic I/O error), and `schedule` called
from inside the handler of the same key replaces the claimed row (`plugins.md` rule 55 says so; the
component harness must expose it).

## 7. Test plan

Local runs go through `fliks-heavy`, as for every Fliks build.

1. **Ported unit tests.** `delivery-queue.test.ts` (5) becomes unit tests of the pure state machine
   (`next(state, outcome) -> Action`): capacity drop of the oldest at 201, unconfigured never queues but
   is logged, non-https refused on the first attempt without retry, six attempts then `failed` with the
   exact delays 1, 5, 15, 30, 60 s, success is final. `manifest.test.ts` (3) becomes `xtask` checks
   (files hashes match, `fliks` range has an upper bound and covers the target core version, shape of
   permissions and policies). `target-guard.test.ts` keeps its two pure cases (unparsable URL, non-https)
   as unit tests; the two literal-IP cases move to the host (below).
2. **Deleted with the transport:** `dispatcher.test.ts` (6), `protocol.test.ts` (6), `harness.test.ts`
   (1, spawned the Node process and played core). The SDK component harness replaces the last one.
3. **Deleted with the guard:** `internal-address.test.ts` (table of addresses plus "not a literal IP")
   and the guard half of `target-guard.test.ts`. Cross-check task: every address of that table must
   appear in the host `address-class` vector set (the always-refused and private classes of
   `plugins.md` "Outbound HTTP guard"); the host list is a superset (it adds 6to4, Teredo,
   documentation ranges), so the check is that nothing is missing, written as a test in the host PR.
4. **Golden vectors from the TS code** (one-off script at `3ec27cf`, JSON committed): `backoff-ladder`
   (n to delay), `delivery-body` (the JSON the target receives), `delivery-log-entry` (field names and
   status strings per outcome), `host-of` (`unconfigured`, `invalid`, hostnames).
5. **Component tests under the SDK harness:**
   - Spike criterion 1: a target that fails 5 times then succeeds delivers on the sixth attempt with
     the exact backoff (clock controlled); a host restart between two attempts loses nothing; 6 failed
     attempts end `failed:...` and bump the counter; 6 guest traps trip the breaker.
   - 3.2 end to end: refused states do not retry; a denial from the guard (private target without the
     grant) is final; a DNS error is final; 201 enqueues evict the oldest and `cancel-call` it; a
     `payload` over 64 KiB is dropped; a 2xx is final; the POST body and headers match `delivery-body`.
   - Two concurrent `task` instances never double-deliver one id (host claim); a trap after the POST
     re-delivers once with the same id (at-least-once).
   - Config read per attempt: changing `target_url` between attempts retargets the next one.
   - `start` twice is idempotent; the stuck-row sweep marks old pending rows `lost` and a late call still delivers
     them; a call whose row was dropped for capacity mid-attempt neither overwrites nor re-schedules.
   - Routes: `GET /delivery-log` shape (newest first, 100 cap, stats), `POST /test-delivery` 202 and
     the event name rule, 404 elsewhere.
   - `plugin-manifest` vector set: this template re-expressed as a published first-party shape.
6. **Parity names from `plugins.md`:** `plugin-manifest`, `address-class` (cross-check above),
   `plugin-route-match` (two routes), `plugins-config-owner` (F68, the URL is a key of this plugin
   only), `network-not-granted` negative case (notify must not request `network.internal`),
   Rust-only scheduled-call suite (dedupe by key, lease redelivery, persistence across a host restart,
   key replacement from inside a handler, limits). Spike criteria 1 and 3 are exercised by this port.
7. **Gates:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, native tests,
   `cargo build --release --target wasm32-wasip2`, `verify` (world, archive guard, size).

## 8. Packaging and catalog

Archive `fliks.notify-<version>.fkplugin`: `plugin.json` (`pluginApi: 2`, `runtime: "wasm"`,
`fliks: ">=5.0.0 <6.0.0"`, `database: {schema: true, coreRefs: []}`, `files: {plugin.wasm,
logo.svg}`), `plugin.json.sig`, `plugin.wasm`, `logo.svg`; store method, caps of `plugins.md` rule 14.
The plugin repository builds the unsigned archive for dev installs (`plugins.allow_unsigned`); the
`FK_NOTIFY_SIGNING_KEY` path of `scripts/package-archive.ts:96-118` disappears, the catalog workflow
signs.

Catalog repository (`~/Documents/clement/code/fliks-plugin-catalog`), changes to describe, none made
here. The shared ones (compatibility row `2`, `package-plugin.mjs` accepting `runtime: "wasm"` and
`plugin.wasm`) are item 1 and 2 of section 8 of the download spec; for notify:

1. New directory `plugins/fliks.notify/` (it has none): `src/plugin.json`, `src/plugin.wasm`,
   `src/logo.svg`, `README.md` pointing at this repository (as `fliks.download` does), and
   `versions/1.0.0.json` (version decided, D7; `package.json` says 0.1.0, never published):

   ```json
   {
     "id": "fliks.notify", "name": "Notify",
     "description": "Forwards Fliks domain events to an admin-configured HTTPS webhook, with bounded retry.",
     "author": "Fliks", "kind": "process", "version": "1.0.0",
     "pluginApi": 2, "fliks": ">=5.0.0 <6.0.0",
     "zipUrl": "https://github.com/fliks-app/fk-plugin-notify/releases/download/v1.0.0/fliks.notify-1.0.0.fkplugin",
     "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
     "logo": "https://fliks-app.github.io/fliks-plugin-catalog/plugins/fliks.notify/logo.svg"
   }
   ```

   The placeholder holds the version back until the `Package plugin` workflow signs and records the
   hash. `name`, `description`, `author`, `kind`, `logo` are fixed for every later version.
2. Because the plugin was never listed, the catalog card appears for the first time with Fliks 5; no
   existing entry changes and no immutable version is touched.
3. Archive storage (D4): the signed archive is a GitHub Release asset of this repository and `zipUrl`
   points there; the catalog stores no `dist/` archive for it.

Plugin repository after the port: Rust on `main`; the Node code stays in git history (no release tag
exists, so tag the last Node commit before the port lands [assumed]).

## 9. Behaviour changes (deviations)

| # | Today | Port | Why |
|---|---|---|---|
| NT1 | a restart loses the queue ("nothing is stored to disk", README.md:42-43) | pending deliveries and their retry timers survive restarts, upgrades and disable/enable | scheduled calls (rule 55); README line to rewrite |
| NT2 | URL fixed for the process life ("takes effect the next time the plugin restarts", README.md:37); item captured the URL at enqueue | read at every attempt; a queued delivery goes to the current URL; no URL at attempt time is `refused:no target configured` | live config, and the URL (often a secret) is not copied into `plugin_scheduled_calls` |
| NT3 | `id` restarts at 1 each process; log and counters in memory | `bigserial` id stable across restarts; log and counters persist; counters count since install | table `deliveries` |
| NT4 | one delivery at a time, FIFO | up to 2 at once (`task` lane), best-effort order | lane model |
| NT5 | plugin resolves the host and refuses internal addresses itself, then the HTTP client resolves again | the host guard vets one resolution and connects to it (no rebinding window); `target-guard.ts` and `internal-address.ts` deleted; the guest sees only `HTTP-request-denied` (no address in it), so the log text is fixed | one implementation (plan §0 rule 2), F65-style |
| NT6 | no settings page, no field (finding 2) | form page with `target_url`; `en`, `fr` strings | the README already promises it |
| NT7 | payload size unbounded | a payload over 64 KiB is dropped (`dropped:<the host's message>`) | scheduled call limit |
| NT8 | `User-Agent: fliks.notify/0.1.0` constant | `fliks.notify/<crate version>` | single version source |
| NT9 | log ring holds 200 entries including pending ones | prunes terminal rows beyond the newest 200; pending rows are bounded by the 200 cap | persistence |
| NT10 | n/a | rows pending over 3 hours show `failed:lost` (a late call still delivers them) | host may drop a poison call |
| NT11 | `health` detail from memory | from the database; a database outage fails `health` | state is in Postgres |
| NT12 | README "about four minutes" | README corrected in this PR to about two minutes (111 s); behaviour unchanged | finding 3 |
| NT13 | start-up log line carries the target URL [ts:src/plugin.ts:83] | the URL is never logged | `target_url` is a secret (D8) |

## 10. Decisions

Decided by the user after review (numbers shared with the download spec, section 10):

- **D1, D2** n/a here (download only).
- **D3** `fliks.notify` requests no `network.internal`; nothing to carry forward.
- **D4** The signed archive is a GitHub Release asset; the catalog `zipUrl` points there.
- **D5** The SDK crate is `MIT OR Apache-2.0`, so notify stays MIT.
- **D6** Host `migrate` runs multi-statement scripts; host docs corrected elsewhere. Notify has one
  migration.
- **D7** Ships as `1.0.0`, no maintenance branch (there is no earlier published version).
- **D8** Notify choices:
  - It ships for the first time on Fliks 5 (finding 1).
  - It keeps the 6-attempt ladder (1, 5, 15, 30, 60 s, about 111 s). The README is fixed in this PR
    (lines 14-15: "about four minutes" becomes "about two minutes").
  - A DNS failure stays final, as today.
  - `target_url` is a `secret` form field.
  - The delivery log and the test button in the UI come later. **Proposed `[rust-followup]` issue (not
    opened):** "notify: show the delivery log and a test button in the settings UI", which needs a
    `table`-shaped log route response and a form action.
  - The S8 spike ports only the sections that need no UI or catalog (sections 2 to 5 without the config
    page); the page, the catalog entry and the deviations land in a follow-up PR.
