// A 2xx the SDK cannot trust must become one `Unavailable` `invalid_response` — never an `undefined` field, a raw
// TypeError or a value of the wrong type handed to the caller. This test enumerates EVERY HTTP method the client
// exposes (and `getCustomerUsage`), asserts the list below covers them all (a new method without an entry fails the
// test), and for each data method injects the malformed modes: {}, [], null, non-JSON, empty, 204, and — from a
// known-good body — every top-level field set to null and to the wrong type. Void results (DELETEs) tolerate anything.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { Unavailable, createClient, getCustomerUsage } from "../src/index.ts";
import type { Client } from "../src/index.ts";

const KEY = "vk_test_synthetic_validation_0001";
const ID = "req-validation-0001";

// How to call each method, and a MINIMAL valid body: only the required, non-nullable fields the method validates, so
// setting any of them to null or the wrong type must be rejected. `kind: "void"` is a result the SDK discards (a
// DELETE, or a CSV export); it tolerates any body. `kind: "lenient"` returns an unmodelled object: any JSON object is
// accepted, only a non-object (array, null, non-JSON) is rejected.
type Entry = {
  run: (c: Client) => Promise<unknown>;
  good?: Record<string, unknown>;
  kind?: "void" | "lenient";
};
const ENTRY = { id: "led_1", customerId: "c", kind: "debit", credits: -1, createdAt: "t" };
const SUB = { id: "s", customerId: "c", status: "active", plan: { key: "p" } };
const ENDPOINT = { id: "e", url: "https://h.example/x", events: ["*"], status: "enabled" };
const DELIVERY = { id: "d", endpointId: "e", eventId: "ev", status: "sent" };

const REGISTRY: Record<string, Entry> = {
  "customers.upsert": {
    run: (c) => c.customers.upsert({ id: "acme" }),
    good: { id: "cus_1", externalId: "acme", createdAt: "t" },
  },
  "customers.get": {
    run: (c) => c.customers.get({ customer: "acme" }),
    good: { id: "cus_1", externalId: "acme", createdAt: "t" },
  },
  "customers.list": { run: (c) => c.customers.list({ limit: 1 }), good: { data: [] } },
  "credits.ledger": {
    run: (c) => c.credits.ledger({ customer: "c", limit: 1 }),
    good: { data: [] },
  },
  planUsage: {
    run: (c) => c.planUsage(),
    good: {
      plan: { key: "free", name: "Free" },
      period: { start: "a", end: "b" },
      usage: { events: 1 },
      unconfirmedLimits: { confirmed: false },
    },
  },
  "entitlements.check": {
    run: (c) => c.entitlements.check({ customer: "c", feature: "f" }),
    good: { allowed: true, reason: "ok", remaining: 1, required: 0 },
  },
  "usage.record": {
    run: (c) => c.usage.record({ id: ID, customer: "c", meter: "m", quantity: 1 }),
    good: { event: ENTRY, balance: 1 },
  },
  "credits.balance": {
    run: (c) => c.credits.balance({ customer: "c" }),
    good: { credits: 1, consumed: 0, granted: 1 },
  },
  "credits.usage": {
    run: (c) => c.credits.usage({ customer: "c" }),
    good: { customerId: "c", balance: { credits: 1, granted: 1, consumed: 0 }, meters: [] },
  },
  "credits.grant": {
    run: (c) => c.credits.grant({ id: ID, customer: "c", credits: 1 }),
    good: ENTRY,
  },
  "plans.upsert": {
    run: (c) =>
      c.plans.upsert({ key: "p", name: "P", includedCredits: 1, meter: "m", features: [] }),
    good: { key: "p", name: "P", includedCredits: 1, meter: "m", features: [] },
  },
  "plans.subscribe": { run: (c) => c.plans.subscribe({ customer: "c", plan: "p" }), good: SUB },
  "plans.change": {
    run: (c) => c.plans.change({ id: ID, customer: "c", plan: "p" }),
    good: { subscription: SUB },
  },
  "plans.upsertMeter": {
    run: (c) => c.plans.upsertMeter({ key: "m", unitsPerCredit: 1 }),
    kind: "void",
  },
  "plans.cancel": { run: (c) => c.plans.cancel({ customer: "c" }), kind: "void" },
  "plans.cancelPendingChange": {
    run: (c) => c.plans.cancelPendingChange({ customer: "c" }),
    kind: "void",
  },
  "customerTokens.create": {
    run: (c) => c.customerTokens.create({ customer: "c" }),
    good: { id: "cut_1", token: "vct_x", customerId: "c", expiresAt: "t" },
  },
  "customerTokens.revoke": { run: (c) => c.customerTokens.revoke({ id: "cut_1" }), kind: "void" },
  "billing.diagnostics": {
    run: (c) => c.billing.diagnostics(),
    good: { outbox: {}, webhooks: {}, reconciliation: {} },
  },
  "billing.reconciliation.list": {
    run: (c) => c.billing.reconciliation.list(),
    good: { data: [] },
  },
  "billing.reconciliation.run": { run: (c) => c.billing.reconciliation.run(), kind: "lenient" },
  "billing.reconciliation.resend": {
    run: (c) => c.billing.reconciliation.resend({ id: "rci_1" }),
    good: { item: { id: "rci_1" }, requeued: 1 },
  },
  "creditPacks.upsert": {
    run: (c) => c.creditPacks.upsert({ key: "p", name: "P", credits: 1, price: null }),
    good: { key: "p", name: "P", credits: 1 },
  },
  "creditPacks.list": { run: (c) => c.creditPacks.list(), good: { data: [] } },
  "creditPacks.purchase": {
    run: (c) => c.creditPacks.purchase({ id: ID, customer: "c", pack: "p" }),
    good: { entry: ENTRY, balance: 1 },
  },
  "recipes.list": { run: (c) => c.recipes.list(), good: { data: [] } },
  "recipes.apply": {
    run: (c) => c.recipes.apply({ recipe: "r" }),
    good: { recipe: { id: "r" }, plan: {}, created: {} },
  },
  "webhooks.endpoints.create": {
    run: (c) => c.webhooks.endpoints.create({ url: "https://h.example/x" }),
    good: { ...ENDPOINT, secret: "whsec_synthetic_x" },
  },
  "webhooks.endpoints.list": { run: (c) => c.webhooks.endpoints.list(), good: { data: [] } },
  "webhooks.endpoints.update": {
    run: (c) => c.webhooks.endpoints.update({ id: "e", status: "disabled" }),
    good: ENDPOINT,
  },
  "webhooks.endpoints.rotateSecret": {
    run: (c) => c.webhooks.endpoints.rotateSecret({ id: "e" }),
    good: { ...ENDPOINT, secret: "whsec_synthetic_x" },
  },
  "webhooks.endpoints.test": { run: (c) => c.webhooks.endpoints.test({ id: "e" }), good: DELIVERY },
  "webhooks.endpoints.delete": {
    run: (c) => c.webhooks.endpoints.delete({ id: "e" }),
    kind: "void",
  },
  "webhooks.deliveries.list": {
    run: (c) => c.webhooks.deliveries.list(),
    good: { data: [] },
  },
  "webhooks.deliveries.get": {
    run: (c) => c.webhooks.deliveries.get({ id: "d" }),
    good: { ...DELIVERY, event: { id: "ev", type: "x", createdAt: "t", data: {} }, attemptLog: [] },
  },
  "webhooks.deliveries.redeliver": {
    run: (c) => c.webhooks.deliveries.redeliver({ id: "d" }),
    good: { ...DELIVERY, event: { id: "ev", type: "x", createdAt: "t", data: {} }, attemptLog: [] },
  },
  "activity.list": {
    run: (c) => c.activity.list(),
    good: { data: [], retentionDays: 7 },
  },
  // exports.* return CSV text, not JSON, and are validated by their own verifier; not a JSON-shape method.
  "exports.ledger": { run: (c) => c.exports.ledger(), kind: "void" },
  "exports.usage": { run: (c) => c.exports.usage(), kind: "void" },
  "exports.customers": { run: (c) => c.exports.customers(), kind: "void" },
};

// a value of a type the field never accepts (a string where a number is promised, and so on)
function wrongType(v: unknown): unknown {
  if (typeof v === "number") return "not-a-number";
  if (Array.isArray(v)) return 123;
  if (typeof v === "object" && v !== null) return [];
  return 123; // string, boolean: a number is wrong
}

// every function leaf under the client, as a dotted path
function methodPaths(obj: unknown, prefix = ""): string[] {
  if (typeof obj !== "object" || obj === null) return [];
  const out: string[] = [];
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "function") out.push(path);
    else if (typeof v === "object" && v !== null) out.push(...methodPaths(v, path));
  }
  return out;
}

// a transport that answers every request with the current `response`
let response: () => Response;
const client = () =>
  createClient({ apiKey: KEY, baseUrl: "http://api.invalid", fetch: async () => response() });
const json = (body: unknown, status = 200) =>
  new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: body === undefined ? {} : { "content-type": "application/json" },
  });

describe("response validation", () => {
  it("every client method is covered by the registry (a new method must be added here)", () => {
    const found = methodPaths(client()).sort();
    const listed = Object.keys(REGISTRY).sort();
    assert.deepEqual(found, listed, "client methods and REGISTRY differ");
  });

  it("a malformed 2xx is Unavailable invalid_response for every data method; void methods tolerate it", async () => {
    for (const [name, entry] of Object.entries(REGISTRY)) {
      // a non-object is wrong for any body-returning method; {}/empty/204 are wrong only for a modelled one
      const modes: [string, () => Response][] = [
        ["array", () => json([])],
        ["null", () => json(null)],
        ["non-JSON", () => new Response("<html>proxy</html>", { status: 200 })],
      ];
      if (entry.kind !== "lenient") {
        modes.push(
          ["empty object", () => json({})],
          ["empty body", () => new Response(null, { status: 200 })],
          ["204", () => new Response(null, { status: 204 })],
        );
        for (const [field, value] of Object.entries(entry.good ?? {})) {
          modes.push([`${field}=null`, () => json({ ...entry.good, [field]: null })]);
          modes.push([
            `${field}=wrong type`,
            () => json({ ...entry.good, [field]: wrongType(value) }),
          ]);
        }
      }
      for (const [mode, resp] of modes) {
        response = resp;
        if (entry.kind === "void") {
          await entry.run(client()); // discards the result: a malformed body is harmless, must not throw
          continue;
        }
        await assert.rejects(
          entry.run(client()),
          (e: Error) => e instanceof Unavailable && (e as Unavailable).code === "invalid_response",
          `${name} / ${mode} should be invalid_response`,
        );
      }
      // the good body is accepted (no false positives)
      if (entry.good) {
        const body = entry.good;
        response = () =>
          json(body, name.includes("record") || name.includes("purchase") ? 201 : 200);
        await entry.run(client());
      }
    }
  });

  it("getCustomerUsage: a malformed 2xx is invalid_response", async () => {
    for (const body of [{}, [], null, { customer: { id: "c" }, meters: [] }]) {
      await assert.rejects(
        getCustomerUsage({
          baseUrl: "http://api.invalid",
          token: "vct_x",
          fetch: async () => json(body),
        }),
        (e: Error) => e instanceof Unavailable && (e as Unavailable).code === "invalid_response",
      );
    }
    await assert.rejects(
      getCustomerUsage({
        baseUrl: "http://api.invalid",
        token: "vct_x",
        fetch: async () => new Response("<html>", { status: 200 }),
      }),
      (e: Error) => e instanceof Unavailable,
    );
  });

  it("an error with no body takes its code from the status, not internal_error for everything", async () => {
    for (const [status, code] of [
      [401, "unauthenticated"],
      [403, "unauthenticated"],
      [402, "insufficient_credits"],
      [429, "rate_limited"],
      [503, "internal_error"],
      [400, "validation_error"],
    ] as const) {
      response = () => new Response(null, { status });
      await assert.rejects(
        client().customers.upsert({ id: "acme" }),
        (e: { code?: string; status?: number }) => e.code === code && e.status === status,
        `status ${status}`,
      );
    }
  });

  it("a server that accepts and never answers times out instead of hanging", async () => {
    // a fetch that settles only when its signal aborts (the server accepted the connection and never replied). A real
    // fetch holds an open socket, which keeps the event loop alive so the client's (unref'd) timeout timer fires; a
    // bare pending Promise would not, so this keeps a ref'd timer and clears it on abort.
    const hang: typeof fetch = (_u, init) =>
      new Promise((_resolve, reject) => {
        const signal = (init as RequestInit | undefined)?.signal;
        const keepAlive = setTimeout(() => {}, 5000);
        const fail = () => (
          clearTimeout(keepAlive),
          reject(new DOMException("aborted", "AbortError"))
        );
        if (signal?.aborted) fail();
        else signal?.addEventListener("abort", fail, { once: true });
      });
    const c = createClient({
      apiKey: KEY,
      baseUrl: "http://api.invalid",
      timeoutMs: 50,
      maxRetries: 0,
      fetch: hang,
    });
    await assert.rejects(
      c.credits.balance({ customer: "c" }),
      (e: Error) => e instanceof Unavailable,
    );
    await assert.rejects(
      getCustomerUsage({
        baseUrl: "http://api.invalid",
        token: "vct_x",
        timeoutMs: 50,
        fetch: hang,
      }),
      (e: Error) => e instanceof Unavailable,
    );
  });
});
