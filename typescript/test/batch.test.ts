// Usage through the batch route. The answers below are the ones the route documents (apps/api/openapi.yaml,
// recordUsageBatch): 200 with one result per event in order, `unavailable` for an event that was not recorded, `plan_limit_reached`
// with `planLimit`, a whole-batch 503 with Retry-After, 404 while the route is off. The end-to-end run against the real route is
// apps/api/test/sdk-batch.test.ts.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  AuthError,
  InvalidRequest,
  LimitReached,
  Unavailable,
  VunlyError,
  createClient,
  type BatchEvent,
  type BatchEventResult,
} from "../src/index.ts";

const KEY = "vk_test_synthetic_sdk_batch_0001";
const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers });
const err = (status: number, code: string, details?: Record<string, unknown>, headers = {}) =>
  json(
    status,
    { error: { code, message: code, requestId: "req_synthetic", ...(details ? { details } : {}) } },
    headers,
  );

type Sent = {
  events: { id: string; customerId: string; meter: string; quantity: number; occurredAt: string }[];
};

/** A client whose fetch is `handler`; records every request body and every wait the SDK asked for. */
function harness(
  handler: (n: number, body: Sent, init: RequestInit) => Response | Promise<Response>,
  opts: { maxRetries?: number } = {},
) {
  const bodies: Sent[] = [];
  const waits: number[] = [];
  const headers: Record<string, string>[] = [];
  const c = createClient({
    apiKey: KEY,
    baseUrl: "http://api.invalid",
    maxRetries: opts.maxRetries ?? 0,
    sleep: async (ms) => void waits.push(ms),
    fetch: async (url, init) => {
      assert.equal(new URL(String(url)).pathname, "/v1/usage-events:batch");
      assert.equal(init?.method, "POST");
      const body = JSON.parse(String(init?.body)) as Sent;
      bodies.push(body);
      headers.push(Object.fromEntries(new Headers(init?.headers).entries()));
      return handler(bodies.length, body, init ?? {});
    },
  });
  return { c, bodies, waits, headers };
}

let n = 0;
const ev = (over: Partial<BatchEvent> = {}): BatchEvent => ({
  id: `evt_sdk_${String(n++).padStart(6, "0")}`,
  customer: "cus_abc123",
  meter: "tokens",
  quantity: 5,
  ...over,
});
const ok = (e: { id: string }, status = "accepted", balance = 90): BatchEventResult => ({
  id: e.id,
  status: status as BatchEventResult["status"],
  balance,
});
const reply = (body: Sent, status: (i: number) => string = () => "accepted") =>
  json(200, { results: body.events.map((e, i) => ({ id: e.id, status: status(i), balance: 90 })) });

describe("recordBatch: one request, one result per event, in order", () => {
  it("sends the events as the route documents them and returns the per-event results", async () => {
    const h = harness((_, b) => reply(b, (i) => (i === 1 ? "duplicate" : "accepted")));
    const events = [ev(), ev(), ev({ occurredAt: new Date("2026-11-01T10:00:00+02:00") })];
    const r = await h.c.usage.recordBatch(events);
    assert.deepEqual(
      r.results.map((x) => x.status),
      ["accepted", "duplicate", "accepted"],
    );
    assert.deepEqual(
      r.results.map((x) => x.id),
      events.map((e) => e.id),
    );
    assert.equal(r.planOverage, false);
    assert.equal(r.planLimit, undefined);
    const sent = h.bodies[0]!.events;
    assert.deepEqual(Object.keys(sent[0]!).sort(), [
      "customerId",
      "id",
      "meter",
      "occurredAt",
      "quantity",
    ]);
    assert.equal(sent[0]!.customerId, "cus_abc123");
    assert.equal(
      sent[2]!.occurredAt,
      "2026-11-01T08:00:00.000Z",
      "an offset date is sent as UTC, the same instant",
    );
    assert.match(
      sent[0]!.occurredAt,
      /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/,
      "stamped when omitted",
    );
    assert.equal(h.headers[0]!["authorization"], `Bearer ${KEY}`);
    assert.equal(
      h.headers[0]!["idempotency-key"],
      undefined,
      "no request-level key: each event carries its own id",
    );
  });

  it("surfaces plan_limit_reached with planLimit, and the overage flag of a paid plan", async () => {
    const limit = {
      reason: "plan_included_volume",
      plan: "free",
      includedMonthlyVolume: 10,
      used: 10,
    };
    const h = harness((_, b) =>
      json(200, {
        results: b.events.map((e) => ({ id: e.id, status: "plan_limit_reached" })),
        planLimit: limit,
      }),
    );
    const r = await h.c.usage.recordBatch([ev()]);
    assert.equal(r.results[0]!.status, "plan_limit_reached");
    assert.deepEqual(r.planLimit, limit);
    const paid = harness((_, b) =>
      json(200, { results: b.events.map((e) => ok(e)) }, { "Vunly-Plan-Overage": "true" }),
    );
    assert.equal((await paid.c.usage.recordBatch([ev()])).planOverage, true);
  });

  it("refuses a bad event, a bad size or a repeated id BEFORE any request", async () => {
    const h = harness(() => assert.fail("no request expected"));
    const bad: [string, () => Promise<unknown>][] = [
      ["id", () => h.c.usage.recordBatch([ev({ id: "short" })])],
      [
        "customer: an external id, not cus_",
        () => h.c.usage.recordBatch([ev({ customer: "my-customer" })]),
      ],
      ["meter", () => h.c.usage.recordBatch([ev({ meter: "Bad Meter" })])],
      ["quantity 0", () => h.c.usage.recordBatch([ev({ quantity: 0 })])],
      ["quantity 1.5", () => h.c.usage.recordBatch([ev({ quantity: 1.5 })])],
      ["quantity too large", () => h.c.usage.recordBatch([ev({ quantity: 1_000_000_001 })])],
      ["occurredAt", () => h.c.usage.recordBatch([ev({ occurredAt: "not a date" })])],
      ["empty", () => h.c.usage.recordBatch([])],
      ["101 events", () => h.c.usage.recordBatch(Array.from({ length: 101 }, () => ev()))],
      [
        "a repeated id",
        () => h.c.usage.recordBatch([ev({ id: "evt_repeated_1" }), ev({ id: "evt_repeated_1" })]),
      ],
    ];
    for (const [what, run] of bad) await assert.rejects(run, InvalidRequest, what);
    assert.equal(h.bodies.length, 0);
  });

  it("100 events are accepted in one request", async () => {
    const h = harness((_, b) => reply(b));
    const r = await h.c.usage.recordBatch(Array.from({ length: 100 }, () => ev()));
    assert.equal(r.results.length, 100);
    assert.equal(h.bodies.length, 1);
  });

  it("results that do not match what was sent are Unavailable invalid_response, never silently trusted", async () => {
    for (const results of [
      [],
      [{ id: "evt_other_1", status: "accepted" }],
      [{ id: "x", status: 1 }],
    ]) {
      const h = harness(() => json(200, { results }));
      await assert.rejects(
        h.c.usage.recordBatch([ev()]),
        (e: unknown) => e instanceof Unavailable && e.code === "invalid_response",
      );
    }
    const noResults = harness(() => json(200, {}));
    await assert.rejects(
      noResults.c.usage.recordBatch([ev()]),
      (e: unknown) => e instanceof Unavailable && e.code === "invalid_response",
    );
  });

  it("a whole-batch 503 is retried with the SAME body after its Retry-After, then surfaces as Unavailable with retryAfterSeconds", async () => {
    const h = harness(
      () =>
        err(
          503,
          "unavailable",
          { reason: "shipping_behind", retryAfterSeconds: 1 },
          { "Retry-After": "1" },
        ),
      { maxRetries: 2 },
    );
    const events = [ev(), ev()];
    await assert.rejects(
      h.c.usage.recordBatch(events),
      (e: unknown) => e instanceof Unavailable && e.status === 503 && e.retryAfterSeconds === 1,
    );
    assert.equal(h.bodies.length, 3, "one send and two retries");
    assert.deepEqual(
      h.bodies[1],
      h.bodies[0],
      "the retry carries the same ids and the same occurredAt",
    );
    assert.deepEqual(h.waits, [1000, 1000], "it waited the Retry-After, not less");
  });

  it("a route that is off (404) says so, still as InvalidRequest not_found", async () => {
    const h = harness(() => err(404, "not_found"));
    await assert.rejects(
      h.c.usage.recordBatch([ev()]),
      (e: unknown) =>
        e instanceof InvalidRequest &&
        e.code === "not_found" &&
        /not available for this project/.test(e.message),
    );
  });

  it("the key never appears in an error", async () => {
    const h = harness(() => err(403, "forbidden"));
    await assert.rejects(
      h.c.usage.recordBatch([ev()]),
      (e: unknown) => e instanceof AuthError && !String((e as Error).message).includes(KEY),
    );
  });
});

describe("batcher: buffer, flush, per-event outcomes", () => {
  it("a full buffer is sent at once as one request, and each add resolves with its own outcome", async () => {
    const h = harness((_, b) => reply(b, (i) => (i === 2 ? "insufficient_credits" : "accepted")));
    const b = h.c.usage.batcher({ maxBatchSize: 3, flushIntervalMs: 0 });
    const out = await Promise.all([b.add(ev()), b.add(ev()), b.add(ev())]);
    assert.deepEqual(
      out.map((x) => x.status),
      ["accepted", "accepted", "insufficient_credits"],
    );
    assert.equal(h.bodies.length, 1);
    assert.equal(h.bodies[0]!.events.length, 3);
    assert.equal(b.pending, 0);
    await b.close();
  });

  it("a partial buffer waits for flush(), or for the interval", async () => {
    const h = harness((_, b) => reply(b));
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const p = b.add(ev());
    await new Promise((r) => setTimeout(r, 20));
    assert.equal(h.bodies.length, 0, "nothing is sent without a flush when the interval is 0");
    assert.equal(b.pending, 1);
    await b.flush();
    assert.equal((await p).status, "accepted");
    const timed = harness((_, bd) => reply(bd));
    const t = timed.c.usage.batcher({ flushIntervalMs: 10 });
    assert.equal((await t.add(ev())).status, "accepted", "the interval sent it");
    await t.close();
  });

  it("an unavailable event is resent alone with the SAME id and occurredAt, after at least Retry-After; the rest are not resent", async () => {
    const h = harness((call, b) => {
      if (call === 1)
        return json(
          200,
          {
            results: b.events.map((e, i) => ({
              id: e.id,
              status: i === 1 ? "unavailable" : "accepted",
              balance: 90,
            })),
          },
          { "Retry-After": "7" },
        );
      return reply(b);
    });
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const ps = [b.add(ev()), b.add(ev()), b.add(ev())];
    await b.flush();
    const out = await Promise.all(ps);
    assert.deepEqual(
      out.map((x) => x.status),
      ["accepted", "accepted", "accepted"],
    );
    assert.equal(h.bodies.length, 2);
    assert.deepEqual(
      h.bodies[1]!.events,
      [h.bodies[0]!.events[1]],
      "only the unavailable one, unchanged",
    );
    assert.equal(h.waits.length, 1);
    assert.ok(h.waits[0]! >= 125, "a wait before the resend (the backoff)");
  });

  it("a whole-batch 503 resends every event after its Retry-After (never earlier), and the final outcome is accepted", async () => {
    const h = harness((call, b) =>
      call === 1
        ? err(
            503,
            "unavailable",
            { reason: "shipping_behind", retryAfterSeconds: 3 },
            { "Retry-After": "3" },
          )
        : reply(b),
    );
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const ps = [b.add(ev()), b.add(ev())];
    await b.flush();
    assert.deepEqual(
      (await Promise.all(ps)).map((x) => x.status),
      ["accepted", "accepted"],
    );
    assert.deepEqual(h.waits, [3000], "waited exactly the Retry-After");
    assert.deepEqual(h.bodies[1], h.bodies[0]);
  });

  it("stops after maxAttempts and surfaces the event as unavailable (not recorded), never retrying forever", async () => {
    const h = harness((_, b) =>
      json(200, { results: b.events.map((e) => ({ id: e.id, status: "unavailable" })) }),
    );
    const b = h.c.usage.batcher({ flushIntervalMs: 0, maxAttempts: 3 });
    const p = b.add(ev());
    await b.flush();
    assert.equal((await p).status, "unavailable");
    assert.equal(h.bodies.length, 3, "three sends in all");
    assert.equal(b.pending, 0);
  });

  it("a whole-batch 503 that never clears also ends as unavailable after maxAttempts", async () => {
    const h = harness(() => err(503, "unavailable", { reason: "shipping_behind" }));
    const b = h.c.usage.batcher({ flushIntervalMs: 0, maxAttempts: 2 });
    const p = b.add(ev());
    await b.flush();
    assert.equal((await p).status, "unavailable");
    assert.equal(h.bodies.length, 2);
  });

  it("a Retry-After longer than the SDK will wait surfaces the events at once, as not recorded", async () => {
    const h = harness(() =>
      err(503, "unavailable", { reason: "shipping_behind" }, { "Retry-After": "3600" }),
    );
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const p = b.add(ev());
    await b.flush();
    assert.equal((await p).status, "unavailable");
    assert.equal(h.bodies.length, 1);
    assert.deepEqual(h.waits, [], "it did not sit on them");
  });

  it("plan_limit_reached, duplicate, too_late, meter_not_found and error are final: never resent", async () => {
    const statuses = ["plan_limit_reached", "duplicate", "too_late", "meter_not_found", "error"];
    const h = harness((_, b) =>
      json(200, { results: b.events.map((e, i) => ({ id: e.id, status: statuses[i] })) }),
    );
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const ps = statuses.map(() => b.add(ev()));
    await b.flush();
    const out = await Promise.all(ps);
    assert.deepEqual(
      out.map((x) => x.status),
      statuses,
    );
    assert.equal(h.bodies.length, 1);
  });

  it("a failure a resend cannot fix rejects each add with the error and does not retry (a bad key, a frozen project)", async () => {
    const auth = harness(() => err(401, "unauthenticated"));
    const a = auth.c.usage.batcher({ flushIntervalMs: 0 });
    const pa = a.add(ev());
    const settled = pa.then(
      () => "resolved",
      (e: unknown) => e,
    );
    await a.flush();
    assert.ok((await settled) instanceof AuthError);
    assert.equal(auth.bodies.length, 1);
    const frozen = harness(() => err(403, "project_frozen"));
    const f = frozen.c.usage.batcher({ flushIntervalMs: 0 });
    const pf = f.add(ev()).then(
      () => "resolved",
      (e: unknown) => e,
    );
    await f.flush();
    const frozenError = await pf;
    assert.ok(frozenError instanceof VunlyError && frozenError.code === "project_frozen");
    assert.equal(frozen.bodies.length, 1);
  });

  it("bounded: past maxPending add rejects with Unavailable instead of queueing without limit", async () => {
    const h = harness((_, b) => reply(b));
    const b = h.c.usage.batcher({ flushIntervalMs: 0, maxPending: 2 });
    const p1 = b.add(ev());
    const p2 = b.add(ev());
    await assert.rejects(b.add(ev()), Unavailable);
    await b.flush();
    await Promise.all([p1, p2]);
    assert.equal(b.pending, 0);
    const after = b.add(ev());
    await b.flush();
    assert.equal((await after).status, "accepted", "room again once the buffer has drained");
  });

  it("the same event added twice is one send and one outcome; a different body under the same id is an idempotency conflict", async () => {
    const h = harness((_, b) => reply(b));
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const e = ev();
    const p1 = b.add(e);
    const p2 = b.add({ ...e });
    assert.equal(p1, p2);
    await assert.rejects(
      b.add({ ...e, quantity: e.quantity + 1 }),
      (x: unknown) => x instanceof InvalidRequest && x.code === "idempotency_conflict",
    );
    await b.flush();
    assert.equal(h.bodies[0]!.events.length, 1);
  });

  it("an invalid event rejects its add and queues nothing", async () => {
    const h = harness(() => assert.fail("no request expected"));
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    await assert.rejects(b.add(ev({ customer: "not-a-cus-id" })), InvalidRequest);
    assert.equal(b.pending, 0);
    await b.close();
  });

  it("close() flushes what is buffered and then refuses new events", async () => {
    const h = harness((_, b) => reply(b));
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const p = b.add(ev());
    await b.close();
    assert.equal((await p).status, "accepted");
    await assert.rejects(
      b.add(ev()),
      (e: unknown) => e instanceof Unavailable && /closed/.test((e as Error).message),
    );
  });

  it("refuses nonsense options", () => {
    const h = harness(() => assert.fail("no request expected"));
    for (const o of [
      { maxBatchSize: 0 },
      { maxBatchSize: 101 },
      { maxPending: 0 },
      { maxAttempts: 0 },
      { flushIntervalMs: -1 },
    ])
      assert.throws(() => h.c.usage.batcher(o), RangeError);
  });

  it("splits more than maxBatchSize events across requests, in order, each at most 100", async () => {
    const h = harness((_, b) => reply(b));
    const b = h.c.usage.batcher({ flushIntervalMs: 0 });
    const events = Array.from({ length: 250 }, () => ev());
    const ps = events.map((e) => b.add(e));
    await b.flush();
    const out = await Promise.all(ps);
    assert.equal(out.length, 250);
    assert.ok(h.bodies.every((x) => x.events.length <= 100));
    assert.deepEqual(
      h.bodies.flatMap((x) => x.events.map((e) => e.id)),
      events.map((e) => e.id),
    );
  });
});
