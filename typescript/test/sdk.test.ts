import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  AuthError,
  InvalidRequest,
  LimitReached,
  PlanLimitReached,
  ProjectFrozen,
  Unavailable,
  VunlyError,
  createClient,
  getCustomerUsage,
  normalizeBaseUrl,
} from "../src/index.ts";

const KEY = "vk_test_synthetic_sdk_unit_0001";
const ENTRY = {
  id: "led_1",
  customerId: "cus_1",
  kind: "debit",
  credits: -1,
  createdAt: "2026-07-01T12:00:00.000Z",
};
const reply = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers });

function client(handler: (url: string, init: RequestInit) => Response | Promise<Response>) {
  const seen: { url: string; init: RequestInit }[] = [];
  const c = createClient({
    apiKey: KEY,
    baseUrl: "http://api.invalid/",
    fetch: async (url, init) => {
      seen.push({ url: String(url), init: init ?? {} });
      return handler(String(url), init ?? {});
    },
  });
  return { c, seen };
}

describe("sdk", () => {
  it("records usage with the id as Idempotency-Key and reports replays", async () => {
    const entry = {
      id: "led_1",
      customerId: "cus_1",
      kind: "debit",
      credits: -3,
      createdAt: "2026-07-01T12:00:00.000Z",
    };
    const { c, seen } = client(() =>
      reply(200, { event: entry, balance: 7 }, { "Idempotent-Replayed": "true" }),
    );
    const r = await c.usage.record({
      id: "req-abc-12345",
      customer: "cus_demo_01",
      meter: "tokens",
      quantity: 3,
    });
    assert.deepEqual([r.remaining, r.replayed], [7, true]);
    const h = seen[0]!.init.headers as Record<string, string>;
    assert.equal(h["Idempotency-Key"], "req-abc-12345");
    assert.equal(h.Authorization, `Bearer ${KEY}`);
    assert.equal(seen[0]!.url, "http://api.invalid/v1/usage-events");
  });

  it("maps balance to remaining and consumed", async () => {
    const { c } = client(() => reply(200, { credits: 4, consumed: 6, granted: 10 }));
    assert.deepEqual(await c.credits.balance({ customer: "cus_demo_01" }), {
      remaining: 4,
      consumed: 6,
      granted: 10,
    });
  });

  it("throws typed errors with stable codes", async () => {
    const err = (status: number, code: string, details?: object) =>
      reply(status, { error: { code, message: "m", requestId: "req_1", details } });
    const run = (status: number, code: string, details?: object) =>
      client(() => err(status, code, details)).c.usage.record({
        id: "req-abc-12345",
        customer: "c",
        meter: "tokens",
        quantity: 1,
      });
    await assert.rejects(
      run(402, "insufficient_credits", { balance: 1, required: 2 }),
      (e: LimitReached) => e instanceof LimitReached && e.details?.required === 2,
    );
    await assert.rejects(
      run(401, "key_revoked"),
      (e: AuthError) => e instanceof AuthError && e.code === "key_revoked",
    );
    await assert.rejects(run(403, "missing_scope"), AuthError);
    await assert.rejects(run(409, "idempotency_conflict"), InvalidRequest);
    await assert.rejects(
      run(404, "meter_not_found"),
      (e: InvalidRequest) =>
        e instanceof InvalidRequest && e.code === "meter_not_found" && e.status === 404,
    );
    await assert.rejects(run(503, "internal_error"), Unavailable);
    await assert.rejects(run(429, "rate_limited"), Unavailable);
  });

  it("entitlements.check posts the estimate and returns the typed decision", async () => {
    const decision = {
      allowed: false,
      reason: "insufficient_credits",
      remaining: 1,
      required: 2,
      plan: "plan_sandbox_hybrid",
    };
    const { c, seen } = client(() => reply(200, decision));
    const r = await c.entitlements.check({
      customer: "cus_demo_01",
      feature: "generate",
      estimate: { meter: "tokens", quantity: 1050 },
    });
    assert.deepEqual(r, decision);
    assert.equal(seen[0]!.url, "http://api.invalid/v1/entitlements/check");
    assert.deepEqual(JSON.parse(String(seen[0]!.init.body)), {
      customerId: "cus_demo_01",
      feature: "generate",
      estimate: { meter: "tokens", quantity: 1050 },
    });
  });

  it("plans.subscribe and cancel use the customer subscription path", async () => {
    const { c, seen } = client((_u, init) =>
      init.method === "DELETE"
        ? new Response(null, { status: 204 })
        : reply(201, {
            id: "sub_1",
            customerId: "cus_demo_01",
            status: "active",
            plan: { key: "plan_sandbox_hybrid" },
          }),
    );
    await c.plans.subscribe({ customer: "cus_demo_01", plan: "plan_sandbox_hybrid" });
    await c.plans.cancel({ customer: "cus_demo_01" });
    assert.deepEqual(
      seen.map((s) => `${s.init.method} ${s.url}`),
      [
        "POST http://api.invalid/v1/customers/cus_demo_01/subscription",
        "DELETE http://api.invalid/v1/customers/cus_demo_01/subscription",
      ],
    );
  });

  it("network failure is Unavailable and never leaks the key", async () => {
    const { c } = client(() => {
      throw new Error(`boom Authorization: Bearer ${KEY}`);
    });
    await assert.rejects(c.customers.upsert({ id: "x" }), (e: Unavailable) => {
      assert.ok(e instanceof Unavailable);
      assert.ok(!e.message.includes(KEY) && !JSON.stringify(e).includes(KEY));
      return true;
    });
  });

  it("rejects a malformed idempotency id client-side, before any request", async () => {
    const { c, seen } = client(() => reply(200, {}));
    for (const id of ["q-1", "short", "has space in it", "x".repeat(129), "emoji-😀-id-0001"]) {
      await assert.rejects(
        c.usage.record({ id, customer: "cus_demo_01", meter: "tokens", quantity: 1 }),
        (e: InvalidRequest) =>
          e instanceof InvalidRequest &&
          e.code === "validation_error" &&
          /8 to 128/.test(e.message),
        id,
      );
    }
    await assert.rejects(
      c.credits.grant({ id: "g-1", customer: "cus_demo_01", credits: 1 }),
      InvalidRequest,
    );
    assert.equal(seen.length, 0, "nothing may be sent for a malformed id");
    // The boundaries are valid: 8 and 128 characters.
    const ok = { event: { ...ENTRY, id: "led_1" }, balance: 0 };
    const { c: c2 } = client(() => reply(201, ok));
    for (const id of ["abcdefgh", "a".repeat(128), "req:2026.10-24_x"]) {
      await c2.usage.record({ id, customer: "cus_demo_01", meter: "tokens", quantity: 1 });
    }
  });

  it("recipes.list and recipes.apply use the recipe endpoints", async () => {
    const { c, seen } = client((_u, init) =>
      init.method === "GET"
        ? reply(200, { data: [{ id: "ai-saas-subscription", version: 1 }] })
        : reply(201, { recipe: { id: "ai-saas-subscription", version: 1 }, plan: {}, created: {} }),
    );
    assert.equal((await c.recipes.list())[0]!.id, "ai-saas-subscription");
    await c.recipes.apply({ recipe: "ai-saas-subscription", params: { includedCredits: 5 } });
    await c.recipes.apply({ recipe: "agent-runs-prepaid" });
    assert.deepEqual(
      seen.map((s) => `${s.init.method} ${s.url}`),
      [
        "GET http://api.invalid/v1/recipes",
        "POST http://api.invalid/v1/recipes/ai-saas-subscription/apply",
        "POST http://api.invalid/v1/recipes/agent-runs-prepaid/apply",
      ],
    );
    assert.deepEqual(JSON.parse(String(seen[1]!.init.body)), { params: { includedCredits: 5 } });
    assert.deepEqual(JSON.parse(String(seen[2]!.init.body)), {});
  });

  it("activity.list queries /v1/activity with the documented parameter names", async () => {
    const { c, seen } = client(() => reply(200, { data: [], nextCursor: null, retentionDays: 7 }));
    const r = await c.activity.list({
      requestId: "req_abc",
      customer: "acme",
      outcome: "error",
      limit: 5,
    });
    assert.equal(r.retentionDays, 7);
    await c.activity.list();
    assert.deepEqual(
      seen.map((s) => s.url.replace("http://api.invalid", "")),
      ["/v1/activity?requestId=req_abc&outcome=error&limit=5&customerId=acme", "/v1/activity"],
    );
  });

  it("plans.change sends the id as the Idempotency-Key and reports replays; cancelPendingChange deletes", async () => {
    const result = {
      change: { id: "chg_1", creditsGranted: 400 },
      subscription: { id: "sub_1", customerId: "acme", status: "active", plan: { key: "pro" } },
    };
    const { c, seen } = client((_u, init) =>
      init.method === "DELETE"
        ? new Response(null, { status: 204 })
        : reply(200, result, { "Idempotent-Replayed": "true" }),
    );
    const r = await c.plans.change({
      id: "chg-sdk-000001",
      customer: "acme",
      plan: "pro",
      timing: "now",
    });
    assert.equal(r.replayed, true);
    assert.equal(r.change?.creditsGranted, 400);
    await c.plans.cancelPendingChange({ customer: "acme" });
    const h = seen[0]!.init.headers as Record<string, string>;
    assert.equal(h["Idempotency-Key"], "chg-sdk-000001");
    assert.deepEqual(JSON.parse(String(seen[0]!.init.body)), { plan: "pro", timing: "now" });
    assert.deepEqual(
      seen.map((s) => `${s.init.method} ${s.url.replace("http://api.invalid", "")}`),
      [
        "POST /v1/customers/acme/subscription/change",
        "DELETE /v1/customers/acme/subscription/pending-change",
      ],
    );
    await assert.rejects(
      c.plans.change({ id: "short", customer: "acme", plan: "pro" }),
      InvalidRequest,
    );
  });

  it("creditPacks use the catalog and purchase routes; purchase sends the id as the Idempotency-Key", async () => {
    const { c, seen } = client((u, init) =>
      u.includes("/purchase")
        ? reply(
            200,
            { entry: { ...ENTRY, id: "led_1", credits: 1000 }, balance: 1000 },
            { "Idempotent-Replayed": "true" },
          )
        : init.method === "GET"
          ? reply(200, { data: [{ key: "small" }] })
          : reply(201, { key: "small", name: "Small", credits: 1000 }),
    );
    await c.creditPacks.upsert({ key: "small", name: "Small", credits: 1000, price: null });
    assert.equal((await c.creditPacks.list())[0]!.key, "small");
    const r = await c.creditPacks.purchase({
      id: "pack-sdk-000001",
      customer: "acme",
      pack: "small",
    });
    assert.deepEqual([r.balance, r.replayed], [1000, true]);
    assert.equal(
      (seen[2]!.init.headers as Record<string, string>)["Idempotency-Key"],
      "pack-sdk-000001",
    );
    assert.deepEqual(
      seen.map((s) => `${s.init.method} ${s.url.replace("http://api.invalid", "")}`),
      [
        "POST /v1/credit-packs",
        "GET /v1/credit-packs",
        "POST /v1/customers/acme/credit-packs/small/purchase",
      ],
    );
    await assert.rejects(
      c.creditPacks.purchase({ id: "bad", customer: "acme", pack: "small" }),
      InvalidRequest,
    );
  });

  it("billing.diagnostics and reconciliation use the documented routes and parameter names", async () => {
    const { c, seen } = client((u, init) =>
      u.endsWith("/diagnostics")
        ? reply(200, { connection: null, outbox: {}, webhooks: {}, reconciliation: {} })
        : init.method === "POST"
          ? reply(200, { item: { id: "rec_1", status: "resolved" }, requeued: 3 })
          : reply(200, { data: [], nextCursor: null }),
    );
    assert.equal((await c.billing.diagnostics()).connection, null);
    await c.billing.reconciliation.list({ status: "open", customer: "acme", limit: 5 });
    const r = await c.billing.reconciliation.resend({ id: "rec_1" });
    assert.deepEqual([r.item.status, r.requeued], ["resolved", 3]);
    await c.billing.reconciliation.run();
    assert.deepEqual(
      seen.map((s) => `${s.init.method} ${s.url.replace("http://api.invalid", "")}`),
      [
        "GET /v1/billing/diagnostics",
        "GET /v1/reconciliation/items?status=open&limit=5&customerId=acme",
        "POST /v1/reconciliation/items/rec_1/resend",
        "POST /v1/reconciliation/run",
      ],
    );
  });

  it("customerTokens mint and revoke on the server; getCustomerUsage reads with the token alone", async () => {
    const { c, seen } = client((u, init) =>
      init.method === "DELETE"
        ? new Response(null, { status: 204 })
        : reply(201, {
            id: "cut_1",
            token: "vct_x",
            customerId: "cus_1",
            expiresAt: "2026-01-01T00:00:00Z",
          }),
    );
    const t = await c.customerTokens.create({ customer: "acme", ttlSeconds: 120 });
    assert.equal(t.token, "vct_x");
    await c.customerTokens.revoke({ id: "cut_1" });
    assert.deepEqual(JSON.parse(String(seen[0]!.init.body)), {
      customerId: "acme",
      ttlSeconds: 120,
    });
    assert.equal(seen[1]!.url, "http://api.invalid/v1/customer-tokens/cut_1");

    const calls: { url: string; auth: string }[] = [];
    const usage = await getCustomerUsage({
      baseUrl: "http://api.invalid/",
      token: "vct_abc",
      fetch: async (url, init) => {
        calls.push({
          url: String(url),
          auth: (init?.headers as Record<string, string>).Authorization!,
        });
        return reply(200, {
          customer: { id: "cus_1" },
          balance: { credits: 7, granted: 10, consumed: 3 },
          meters: [],
        });
      },
    });
    assert.equal(usage.balance.credits, 7);
    assert.deepEqual(calls, [
      { url: "http://api.invalid/v1/customer/usage", auth: "Bearer vct_abc" },
    ]);
    await assert.rejects(
      getCustomerUsage({
        baseUrl: "http://api.invalid",
        token: "vct_old",
        fetch: async () =>
          reply(401, {
            error: { code: "unauthenticated", message: "expired", requestId: "req_1" },
          }),
      }),
      (e: AuthError) => e instanceof AuthError && e.status === 401 && e.requestId === "req_1",
    );
  });

  it("exports return the CSV text and the next cursor, with the documented parameter names", async () => {
    const { c, seen } = client(
      (u) =>
        new Response("id,x\r\na,1\r\n", {
          status: 200,
          headers: {
            "content-type": "text/csv",
            ...(u.includes("limit=2") ? { "x-next-cursor": "abc" } : {}),
          },
        }),
    );
    const a = await c.exports.ledger({
      customer: "acme",
      limit: 2,
      from: "2026-01-01T00:00:00.000Z",
    });
    assert.deepEqual([a.csv, a.nextCursor], ["id,x\r\na,1\r\n", "abc"]);
    const b = await c.exports.customers();
    assert.equal(b.nextCursor, null);
    await c.exports.usage({ cursor: "abc" });
    assert.deepEqual(
      seen.map((s) => s.url.replace("http://api.invalid", "")),
      [
        "/v1/exports/ledger.csv?limit=2&from=2026-01-01T00%3A00%3A00.000Z&customerId=acme",
        "/v1/exports/customers.csv",
        "/v1/exports/usage.csv?cursor=abc",
      ],
    );
  });

  it("refuses an empty key", () => {
    assert.throws(() => createClient({ apiKey: "", baseUrl: "http://x" }));
  });
});

describe("base URL forms", () => {
  const forms = [
    "http://api.invalid",
    "http://api.invalid/",
    "http://api.invalid/v1",
    "http://api.invalid/v1/",
    "  http://api.invalid/v1  ",
    "http://api.invalid//v1//",
  ];
  it("normalizeBaseUrl gives one address for every form, and keeps a real path prefix", () => {
    for (const f of forms)
      assert.equal(normalizeBaseUrl(f), "http://api.invalid", JSON.stringify(f));
    assert.equal(normalizeBaseUrl("https://h.example/proxy"), "https://h.example/proxy");
    assert.equal(normalizeBaseUrl("https://h.example/proxy/v1/"), "https://h.example/proxy");
    assert.equal(normalizeBaseUrl("https://h.example/v10"), "https://h.example/v10");
  });

  it("every form calls the same URL, never /v1/v1", async () => {
    for (const baseUrl of forms) {
      const seen: string[] = [];
      const c = createClient({
        apiKey: KEY,
        baseUrl,
        fetch: async (url) => {
          seen.push(String(url));
          return reply(200, { data: { id: "cus_1", externalId: "a" } });
        },
      });
      await c.customers.upsert({ id: "a" }).catch(() => undefined);
      assert.ok(
        seen.length > 0 && seen.every((u) => u.startsWith("http://api.invalid/v1/customers")),
        `${baseUrl}: ${seen}`,
      );
      assert.ok(
        seen.every((u) => !u.includes("/v1/v1")),
        `${baseUrl}: ${seen}`,
      );
    }
  });

  it("getCustomerUsage accepts the same forms", async () => {
    for (const baseUrl of forms) {
      let url = "";
      await getCustomerUsage({
        baseUrl,
        token: "vct_x",
        fetch: async (u) => {
          url = String(u);
          return reply(200, { balance: { credits: 1, granted: 1, consumed: 0 } });
        },
      }).catch(() => undefined);
      assert.equal(url, "http://api.invalid/v1/customer/usage", baseUrl);
    }
  });

  it("a 'no such route' 404 says to check the API address", async () => {
    const { c } = client(() =>
      reply(404, { error: { code: "not_found", message: "no such route", requestId: "r1" } }),
    );
    await assert.rejects(
      c.customers.upsert({ id: "a" }),
      (e: Error) =>
        e instanceof InvalidRequest &&
        /check the API address/.test(e.message) &&
        /no such route/.test(e.message),
    );
    // a real not_found (a customer that does not exist) is left alone
    const other = client(() =>
      reply(404, { error: { code: "not_found", message: "customer not found", requestId: "r2" } }),
    );
    await assert.rejects(
      other.c.credits.balance({ customer: "zz" }),
      (e: Error) => !/API address/.test(e.message),
    );
  });
});

describe("retries: only what is safe, with the same id", () => {
  const ok = () => reply(201, { event: { ...ENTRY, credits: -2 }, balance: 98 });
  const rec = (c: ReturnType<typeof createClient>) =>
    c.usage.record({ id: "req-abc-12345", customer: "cus_1", meter: "tokens", quantity: 1050 });
  function with_(answers: (Response | Error)[], extra: { maxRetries?: number } = {}) {
    const seen: RequestInit[] = [];
    const slept: number[] = [];
    const c = createClient({
      apiKey: KEY,
      baseUrl: "http://api.invalid",
      ...extra,
      sleep: async (ms) => void slept.push(ms),
      fetch: async (_u, init) => {
        seen.push(init ?? {});
        const a = answers.shift()!;
        if (a instanceof Error) throw a;
        return a;
      },
    });
    return { c, seen, slept };
  }
  const err = (status: number, code: string, headers: Record<string, string> = {}) =>
    reply(status, { error: { code, message: "m", requestId: "req_x" } }, headers);

  it("503 then success: retried with the same Idempotency-Key", async () => {
    const t = with_([err(503, "internal_error"), err(500, "internal_error"), ok()]);
    assert.equal((await rec(t.c)).remaining, 98);
    assert.equal(t.seen.length, 3);
    assert.deepEqual(
      new Set(t.seen.map((i) => (i.headers as Record<string, string>)["Idempotency-Key"])),
      new Set(["req-abc-12345"]),
    );
    assert.equal(t.slept.length, 2);
  });

  it("a network failure is retried; when retries run out it is Unavailable", async () => {
    const down = new TypeError("fetch failed");
    const t = with_([down, down, down]);
    await assert.rejects(
      rec(t.c),
      (e: Error) => e instanceof Unavailable && (e as Unavailable).status === 0,
    );
    assert.equal(t.seen.length, 3);
    const t2 = with_([down, ok()]);
    assert.equal((await rec(t2.c)).remaining, 98);
  });

  it("429 waits Retry-After; a Retry-After over the cap raises instead of hanging", async () => {
    const t = with_([err(429, "rate_limited", { "Retry-After": "2" }), ok()]);
    await rec(t.c);
    assert.deepEqual(t.slept, [2000]);
    const t2 = with_([err(429, "rate_limited", { "Retry-After": "3600" })]);
    await assert.rejects(
      rec(t2.c),
      (e: Error) => e instanceof Unavailable && (e as Unavailable).retryAfterSeconds === 3600,
    );
    assert.equal(t2.seen.length, 1);
    assert.deepEqual(t2.slept, []);
  });

  it("one call is bounded whatever the server says: at most 1 + maxRetries requests, no wait over 30 s (failure-modes.md FM-8)", async () => {
    for (const maxRetries of [0, 1, 2, 4]) {
      const n = maxRetries + 1;
      // the worst case for time: every answer is a 429 asking for the longest wait the SDK will honour
      const slow = with_(
        Array.from({ length: n }, () => err(429, "rate_limited", { "Retry-After": "30" })),
        { maxRetries },
      );
      await assert.rejects(rec(slow.c), (e: Error) => e instanceof Unavailable);
      assert.equal(slow.seen.length, n);
      assert.deepEqual(slow.slept, Array(maxRetries).fill(30_000));
      // the worst case for load: every answer is a 503; waits are the capped, jittered backoff and nothing else
      const down = with_(
        Array.from({ length: n }, () => err(503, "internal_error")),
        { maxRetries },
      );
      await assert.rejects(rec(down.c), (e: Error) => e instanceof Unavailable);
      assert.equal(down.seen.length, n);
      down.slept.forEach((ms, i) => {
        const base = Math.min(8000, 250 * 2 ** i);
        assert.ok(
          ms >= base / 2 && ms <= base,
          `wait ${i + 1} was ${ms} ms, expected ${base / 2}..${base}`,
        );
      });
    }
  });

  it("an answer that breaks while its body arrives is Unavailable and retried like a lost answer; an error status keeps its class (failure-modes.md FM-8)", async () => {
    const broken = (status: number) =>
      new Response(
        new ReadableStream({
          pull(controller) {
            controller.error(new TypeError("terminated"));
          },
        }),
        { status, headers: { "content-type": "application/json" } },
      );
    // a 201 whose body never arrives: retried with the same key, and the second answer wins
    const t = with_([broken(201), ok()]);
    assert.equal((await rec(t.c)).remaining, 98);
    assert.equal(t.seen.length, 2);
    assert.deepEqual(
      new Set(t.seen.map((i) => (i.headers as Record<string, string>)["Idempotency-Key"])),
      new Set(["req-abc-12345"]),
    );
    // with retries off it is the SDK's error, never a bare DOMException or TypeError from the runtime
    const t0 = with_([broken(201)], { maxRetries: 0 });
    await assert.rejects(
      rec(t0.c),
      (e: Error) => e instanceof Unavailable && (e as Unavailable).status === 0,
    );
    // a 402 whose body breaks is still a 402
    const t2 = with_([broken(402)]);
    await assert.rejects(rec(t2.c), (e: Error) => e instanceof LimitReached);
    assert.equal(t2.seen.length, 1);
  });

  it("a response that never stops is cut at 16 MB: Unavailable invalid_response, not retried, never held in memory (failure-modes.md R-FM-2)", async () => {
    const chunk = new Uint8Array(1 << 20).fill(0x20);
    const endless = (status: number, pulled: { n: number }) =>
      new Response(
        new ReadableStream({
          pull(controller) {
            pulled.n++;
            controller.enqueue(chunk);
          },
        }),
        { status, headers: { "content-type": "application/json" } },
      );
    const pulled = { n: 0 };
    const t = with_([endless(201, pulled)]);
    await assert.rejects(
      rec(t.c),
      (e: Error) =>
        e instanceof Unavailable &&
        (e as Unavailable).code === "invalid_response" &&
        (e as Unavailable).status === 201,
    );
    assert.equal(t.seen.length, 1, "an oversized success is not retried");
    // the cap plus what the stream had queued ahead of the reader, never the whole (endless) body
    assert.ok(pulled.n >= 17 && pulled.n <= 20, `pulled ${pulled.n} MB`);
    // an error status with an oversized body is still classified by its status
    const t2 = with_([endless(402, { n: 0 })]);
    await assert.rejects(rec(t2.c), (e: Error) => e instanceof LimitReached);
    // a body just under the cap still arrives whole
    const big = JSON.stringify({ id: "x".repeat(15 * (1 << 20)) });
    assert.ok(big.length < 16 * (1 << 20));
    const t3 = with_([
      new Response(big, { status: 200, headers: { "content-type": "application/json" } }),
    ]);
    await assert.rejects(rec(t3.c), (e: Error) => e instanceof Unavailable); // 15 MB is read; it is the shape that is wrong, not the size
    assert.equal(t3.seen.length, 1);
  });

  it("reads Retry-After by the shared vectors: a huge or unrepresentable number is a very long wait, never an early retry", async () => {
    const file = JSON.parse(
      readFileSync(new URL("./retry-after-vectors.json", import.meta.url), "utf8"),
    ) as {
      vectors: { header: string; kind: "seconds" | "surface" | "ignored"; seconds?: number }[];
    };
    assert.ok(file.vectors.length >= 40);
    for (const v of file.vectors) {
      if (!/^[\x20-\x7e]*$/.test(v.header)) continue; // a Headers object cannot carry non-ASCII; the pattern is ASCII-only by construction
      const shown = v.header.length > 30 ? `${v.header.slice(0, 30)}...` : v.header;
      const t = with_([err(429, "rate_limited", { "Retry-After": v.header }), ok()]);
      if (v.kind === "surface") {
        await assert.rejects(
          rec(t.c),
          (e: Error) =>
            e instanceof Unavailable && (e as Unavailable).retryAfterSeconds === v.seconds,
          shown,
        );
        assert.equal(t.seen.length, 1, `Retry-After '${shown}' was retried early`);
        assert.deepEqual(t.slept, [], shown);
      } else {
        assert.equal((await rec(t.c)).remaining, 98, shown);
        assert.equal(t.seen.length, 2, shown);
        if (v.kind === "seconds" && (v.seconds ?? 0) > 0)
          assert.deepEqual(t.slept, [Math.round((v.seconds ?? 0) * 1000)], shown);
        else
          assert.ok(
            t.slept[0]! >= 125 && t.slept[0]! <= 250,
            `Retry-After '${shown}': no directive means the normal backoff, slept ${t.slept}`,
          );
      }
    }
  });

  it("402, 400, 401, 403, 404 and 409 are never retried", async () => {
    for (const [status, code, cls] of [
      [402, "insufficient_credits", LimitReached],
      [400, "validation_error", InvalidRequest],
      [401, "unauthenticated", AuthError],
      [403, "missing_scope", AuthError],
      [404, "not_found", InvalidRequest],
      [409, "idempotency_conflict", InvalidRequest],
    ] as const) {
      const t = with_([err(status, code)]);
      await assert.rejects(rec(t.c), (e: Error) => e instanceof cls);
      assert.equal(t.seen.length, 1, String(status));
    }
  });

  it("maxRetries: 0 turns retries off", async () => {
    const t = with_([err(503, "internal_error")], { maxRetries: 0 });
    await assert.rejects(rec(t.c), Unavailable);
    assert.equal(t.seen.length, 1);
  });

  it("a 2xx response that lacks a promised field is a VunlyError, not undefined, and is not retried", async () => {
    const entry = { id: "led_1", customerId: "cus_1", kind: "debit", credits: -3 }; // no createdAt
    const bad: [string, (c: ReturnType<typeof createClient>) => Promise<unknown>, unknown][] = [
      [
        "usage.record",
        (c) => c.usage.record({ id: "req-abc-12345", customer: "c", meter: "m", quantity: 1 }),
        { event: entry, balance: 7 },
      ],
      [
        "customers.upsert",
        (c) => c.customers.upsert({ id: "acme" }),
        { id: "cus_1", externalId: "acme" },
      ],
      [
        "entitlements.check",
        (c) => c.entitlements.check({ customer: "c", feature: "f" }),
        { allowed: true, reason: "ok" },
      ],
      ["credits.balance", (c) => c.credits.balance({ customer: "c" }), { credits: 1 }],
      ["body is not an object", (c) => c.credits.balance({ customer: "c" }), []],
      [
        "credits.usage without balance",
        (c) => c.credits.usage({ customer: "c" }),
        { customerId: "cus_1", meters: [] },
      ],
      ["credits.usage, null body", (c) => c.credits.usage({ customer: "c" }), null],
    ];
    for (const [name, call, body] of bad) {
      const { c, seen } = client(() => reply(200, body));
      await assert.rejects(
        call(c),
        (e: Error) =>
          e instanceof Unavailable &&
          (e as Unavailable).code === "invalid_response" &&
          /malformed/.test(e.message),
        name,
      );
      assert.equal(seen.length, 1, `${name}: not retried`);
    }
  });

  it("credits.usage with a non-JSON 2xx is invalid_response, not undefined", async () => {
    const { c } = client(() => new Response("<html>proxy</html>", { status: 200 }));
    await assert.rejects(
      c.credits.usage({ customer: "c" }),
      (e: Unavailable) => e instanceof Unavailable && e.code === "invalid_response",
    );
  });

  it("getCustomerUsage: a malformed or non-JSON 2xx is invalid_response; errors keep their class and code", async () => {
    const read = (res: () => Response) =>
      getCustomerUsage({ baseUrl: "http://api.invalid", token: "vct_x", fetch: async () => res() });
    for (const res of [
      () => reply(200, { customer: { id: "cus_1" }, meters: [] }), // no balance
      () => new Response("<html>proxy</html>", { status: 200 }),
      () => new Response(null, { status: 200 }),
    ])
      await assert.rejects(
        read(res),
        (e: Unavailable) => e instanceof Unavailable && e.code === "invalid_response",
      );
    // an error without a body: the class and code follow the status, never "unauthenticated" for a 5xx
    await assert.rejects(
      read(() => new Response(null, { status: 503 })),
      (e: Unavailable) =>
        e instanceof Unavailable && e.code === "internal_error" && e.status === 503,
    );
    await assert.rejects(
      read(() => new Response(null, { status: 429 })),
      (e: Unavailable) => e instanceof Unavailable && e.code === "rate_limited",
    );
    await assert.rejects(
      read(() => new Response(null, { status: 401 })),
      (e: AuthError) => e instanceof AuthError && e.code === "unauthenticated",
    );
  });
});

describe("retry classes (conformance/README.md)", () => {
  const TOKEN = { id: "cut_1", token: "vct_x", customerId: "cus_1", expiresAt: "t" };
  function with_(answers: (Response | Error)[]) {
    const seen: { url: string; init: RequestInit }[] = [];
    const slept: number[] = [];
    const c = createClient({
      apiKey: KEY,
      baseUrl: "http://api.invalid",
      sleep: async (ms) => void slept.push(ms),
      fetch: async (u, init) => {
        seen.push({ url: String(u), init: init ?? {} });
        const a = answers.shift()!;
        if (a instanceof Error) throw a;
        return a;
      },
    });
    return { c, seen, slept };
  }
  const err = (
    status: number,
    code: string,
    headers: Record<string, string> = {},
    details?: object,
  ) =>
    reply(
      status,
      { error: { code, message: "m", requestId: "req_x", ...(details ? { details } : {}) } },
      headers,
    );

  it("class X (a new token every call, no key) is not repeated after a lost answer or a 5xx", async () => {
    const lost = with_([new TypeError("fetch failed"), reply(201, TOKEN)]);
    await assert.rejects(
      lost.c.customerTokens.create({ customer: "cus_1" }),
      (e: Unavailable) => e instanceof Unavailable && e.status === 0,
    );
    assert.equal(lost.seen.length, 1);
    assert.deepEqual(lost.slept, []);
    const five = with_([err(503, "internal_error"), reply(201, TOKEN)]);
    await assert.rejects(five.c.customerTokens.create({ customer: "cus_1" }), Unavailable);
    assert.equal(five.seen.length, 1);
  });

  it("class X is repeated after a 429: it was refused before it ran", async () => {
    const t = with_([err(429, "rate_limited", { "Retry-After": "1" }), reply(201, TOKEN)]);
    assert.equal((await t.c.customerTokens.create({ customer: "cus_1" })).id, "cut_1");
    assert.equal(t.seen.length, 2);
    assert.deepEqual(t.slept, [1000]);
  });

  it("classes N and R are repeated after a lost answer: upsert, subscribe (same plan), read, check", async () => {
    const cust = { id: "cus_1", externalId: "acme", createdAt: "t" };
    const up = with_([new TypeError("fetch failed"), reply(200, cust)]);
    assert.equal((await up.c.customers.upsert({ id: "acme" })).externalId, "acme");
    assert.equal(up.seen.length, 2);
    const sub = with_([
      new TypeError("fetch failed"),
      reply(200, {
        id: "sub_1",
        customerId: "cus_1",
        status: "active",
        plan: { key: "p", name: "P" },
        periodStart: "a",
        periodEnd: "b",
      }),
    ]);
    await sub.c.plans.subscribe({ customer: "acme", plan: "p" });
    assert.equal(sub.seen.length, 2);
    const bal = with_([
      new TypeError("fetch failed"),
      reply(200, { credits: 1, consumed: 0, granted: 1 }),
    ]);
    assert.equal((await bal.c.credits.balance({ customer: "acme" })).remaining, 1);
    assert.equal(bal.seen.length, 2);
    const chk = with_([
      new TypeError("fetch failed"),
      reply(200, { allowed: true, reason: "ok", remaining: 1, required: 0 }),
    ]);
    assert.equal(
      (await chk.c.entitlements.check({ customer: "acme", feature: "f" })).allowed,
      true,
    );
    assert.equal(chk.seen.length, 2);
  });

  // The plan's volume stop is `plan_limit_reached` (402), a non-retryable PlanLimitReached, not
  // Unavailable; a 429 stays a rate limit. project_frozen is its own class, not an AuthError.
  const REC = { id: "req-abc-12345", customer: "c", meter: "tokens", quantity: 1 };
  const stopDetails = {
    reason: "plan_included_volume",
    plan: "free",
    resetsAt: "2026-12-01T00:00:00.000Z",
  };

  it("the plan-volume stop (402 plan_limit_reached) is a PlanLimitReached: never waited on or repeated, details kept", async () => {
    const t = with_([
      err(402, "plan_limit_reached", {}, stopDetails),
      reply(201, { event: ENTRY, balance: 1 }),
    ]);
    await assert.rejects(t.c.usage.record(REC), (e: PlanLimitReached) => {
      assert.ok(e instanceof PlanLimitReached && e instanceof VunlyError);
      assert.ok(!(e instanceof Unavailable) && !(e instanceof LimitReached));
      assert.equal(e.code, "plan_limit_reached");
      assert.equal(e.status, 402);
      assert.equal(e.details?.["resetsAt"], "2026-12-01T00:00:00.000Z");
      return true;
    });
    assert.equal(t.seen.length, 1, "sent once");
    assert.deepEqual(t.slept, [], "no waiting");
  });

  it("an older server's plan stop (429 rate_limited with reason plan_included_volume) is classified the same way", async () => {
    const t = with_([
      err(429, "rate_limited", { "Retry-After": "0" }, stopDetails),
      reply(201, { event: ENTRY, balance: 1 }),
    ]);
    await assert.rejects(t.c.usage.record(REC), PlanLimitReached);
    assert.equal(t.seen.length, 1);
    assert.deepEqual(t.slept, []);
  });

  it("insufficient_credits stays LimitReached and a real 429 stays Unavailable", async () => {
    await assert.rejects(
      with_([err(402, "insufficient_credits", {}, { balance: 0, required: 3 })]).c.usage.record(
        REC,
      ),
      (e: unknown) => e instanceof LimitReached && !(e instanceof PlanLimitReached),
    );
    await assert.rejects(
      with_(
        Array.from({ length: 3 }, () =>
          err(429, "rate_limited", { "Retry-After": "1" }, { retryAfterSeconds: 1 }),
        ),
      ).c.usage.record(REC),
      Unavailable,
    );
  });

  it("project_frozen (403) is a ProjectFrozen, not an AuthError; any other 403 is still an AuthError", async () => {
    await assert.rejects(
      with_([err(403, "project_frozen", {}, { reason: "project_frozen" })]).c.usage.record(REC),
      (e: unknown) =>
        e instanceof ProjectFrozen &&
        !(e instanceof AuthError) &&
        (e as ProjectFrozen).code === "project_frozen" &&
        (e as ProjectFrozen).status === 403,
    );
    await assert.rejects(
      with_([err(403, "missing_scope")]).c.usage.record(REC),
      (e: unknown) => e instanceof AuthError && !(e instanceof ProjectFrozen),
    );
  });

  it("the overage header is surfaced; a replay is never an overage", async () => {
    const body = { event: ENTRY, balance: 1 };
    const t = with_([
      reply(201, body, { "Vunly-Plan-Overage": "true" }),
      reply(200, body, { "Vunly-Plan-Overage": "true", "Idempotent-Replayed": "true" }),
      reply(201, body),
    ]);
    const rec = () =>
      t.c.usage.record({ id: "req-abc-12345", customer: "c", meter: "tokens", quantity: 1 });
    assert.equal((await rec()).planOverage, true);
    assert.equal((await rec()).planOverage, false);
    assert.equal((await rec()).planOverage, false);
  });

  it("every request names the SDK, and the key goes only in Authorization", async () => {
    const t = with_([reply(200, { credits: 1, consumed: 0, granted: 1 })]);
    await t.c.credits.balance({ customer: "acme" });
    const h = t.seen[0]!.init.headers as Record<string, string>;
    assert.match(h["User-Agent"]!, /^vunly-js\/\d+\.\d+\.\d+/);
    assert.equal(h["Authorization"], `Bearer ${KEY}`);
  });

  it("lists and plan usage: pages carry nextCursor, query parameters are the documented ones", async () => {
    const usage = {
      plan: { key: "free", name: "Free" },
      period: { start: "a", end: "b" },
      usage: { events: 3 },
      unconfirmedLimits: {
        confirmed: false,
        note: "n",
        includedMonthlyEvents: 10,
        activityRetentionDays: 7,
      },
    };
    const t = with_([
      reply(200, { data: [ENTRY], nextCursor: "abc" }),
      reply(200, { data: [], nextCursor: null }),
      reply(200, { data: [{ id: "cus_1", externalId: "a", createdAt: "t" }], nextCursor: null }),
      reply(200, usage),
    ]);
    const p1 = await t.c.credits.ledger({ customer: "acme", limit: 1 });
    assert.deepEqual([p1.data.length, p1.nextCursor], [1, "abc"]);
    assert.equal(t.seen[0]!.url, "http://api.invalid/v1/customers/acme/ledger?limit=1");
    const p2 = await t.c.credits.ledger({ customer: "acme", limit: 1, cursor: p1.nextCursor! });
    assert.deepEqual([p2.data.length, p2.nextCursor], [0, null]);
    assert.match(t.seen[1]!.url, /cursor=abc/);
    assert.equal((await t.c.customers.list()).data.length, 1);
    const u = await t.c.planUsage();
    assert.deepEqual(
      [u.plan.key, u.usage.events, u.unconfirmedLimits.confirmed],
      ["free", 3, false],
    );
  });
});
