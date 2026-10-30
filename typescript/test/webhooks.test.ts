import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { describe, it } from "node:test";
import { WebhookSignatureError, createClient, verifyWebhook } from "../src/index.ts";

// A fixed vector computed once with node:crypto (HMAC-SHA256 of `${t}.${body}`), so the format cannot drift silently.
const SECRET = "whsec_test_vector_not_a_secret";
const T = 1_800_000_000;
const BODY =
  '{"id":"evt_0123456789abcdef0123456789abcdef","type":"credits.low","createdAt":"2026-04-01T12:00:00.000Z","environment":"sandbox","data":{"customerId":"cus_1","externalId":"acme","balance":9,"granted":100,"threshold":0.1}}';
const SIG = "3992c2e392cd7d38bb51e7e0fa3e6dbd2f13e17cb12524ba328e4fcc529a26f3";
const HEADER = `t=${T},v1=${SIG}`;
const NOW = T * 1000;

const reason = async (p: Promise<unknown>) => {
  try {
    await p;
  } catch (e) {
    assert.ok(e instanceof WebhookSignatureError, String(e));
    return e.reason;
  }
  return "accepted";
};

describe("verifyWebhook", () => {
  it("accepts the fixed vector and returns the typed event", async () => {
    const event = await verifyWebhook({
      payload: BODY,
      signature: HEADER,
      secret: SECRET,
      now: NOW,
    });
    assert.equal(event.id, "evt_0123456789abcdef0123456789abcdef");
    assert.equal(event.type, "credits.low");
    if (event.type === "credits.low") assert.equal(event.data.balance, 9);
  });

  it("matches what node:crypto computes for any body", async () => {
    const body = JSON.stringify({
      id: "evt_1",
      type: "webhook.test",
      data: { endpointId: "whe_1" },
    });
    const sig = createHmac("sha256", SECRET).update(`${T}.${body}`).digest("hex");
    const e = await verifyWebhook({
      payload: body,
      signature: `t=${T},v1=${sig}`,
      secret: SECRET,
      now: NOW,
    });
    assert.equal(e.id, "evt_1");
  });

  it("rejects a changed body, a wrong secret, a stale or future timestamp and bad headers", async () => {
    const base = { payload: BODY, signature: HEADER, secret: SECRET, now: NOW };
    assert.equal(
      await reason(verifyWebhook({ ...base, payload: BODY + " " })),
      "no_matching_signature",
    );
    assert.equal(
      await reason(verifyWebhook({ ...base, secret: "whsec_synthetic_b" })),
      "no_matching_signature",
    );
    assert.equal(await reason(verifyWebhook({ ...base, now: NOW + 301_000 })), "stale_timestamp");
    assert.equal(await reason(verifyWebhook({ ...base, now: NOW - 301_000 })), "stale_timestamp");
    assert.equal(await reason(verifyWebhook({ ...base, now: NOW + 299_000 })), "accepted");
    assert.equal(
      await reason(verifyWebhook({ ...base, toleranceSeconds: 10, now: NOW + 11_000 })),
      "stale_timestamp",
    );
    assert.equal(await reason(verifyWebhook({ ...base, signature: null })), "missing_header");
    assert.equal(
      await reason(verifyWebhook({ ...base, signature: "garbage" })),
      "malformed_header",
    );
    assert.equal(
      await reason(verifyWebhook({ ...base, signature: `t=${T},v1=abc` })),
      "malformed_header",
    );
    await assert.rejects(verifyWebhook({ ...base, toleranceSeconds: 0 }), /positive/);
  });

  it("accepts any live secret during a rotation and any of several signatures", async () => {
    const base = { payload: BODY, signature: HEADER, now: NOW };
    assert.equal(
      await reason(verifyWebhook({ ...base, secret: ["whsec_synthetic_old", SECRET] })),
      "accepted",
    );
    const two = `t=${T},v1=${"0".repeat(64)},v1=${SIG}`;
    assert.equal(
      await reason(verifyWebhook({ ...base, signature: two, secret: SECRET })),
      "accepted",
    );
  });

  it("refuses an empty or missing secret, alone or inside a rotation list, instead of accepting a forged signature", async () => {
    // the signature an attacker computes when the secret is the empty string
    const forged = createHmac("sha256", "").update(`${T}.${BODY}`).digest("hex");
    for (const secret of [
      "",
      "   ",
      [],
      [""],
      [SECRET, ""],
      [SECRET, " "],
      undefined as unknown as string,
      null as unknown as string,
    ]) {
      await assert.rejects(
        verifyWebhook({ payload: BODY, signature: `t=${T},v1=${forged}`, secret, now: NOW }),
        (e: Error) =>
          !(e instanceof WebhookSignatureError) && /secret is empty or missing/.test(e.message),
        JSON.stringify(secret),
      );
    }
    // a real secret still works, alone and in a list
    assert.equal(
      (
        await verifyWebhook({ payload: BODY, signature: HEADER, secret: [SECRET], now: NOW })
      ).id.startsWith("evt_"),
      true,
    );
  });

  it("refuses a validly signed body that is not an event", async () => {
    const body = "not json";
    const sig = createHmac("sha256", SECRET).update(`${T}.${body}`).digest("hex");
    assert.equal(
      await reason(
        verifyWebhook({ payload: body, signature: `t=${T},v1=${sig}`, secret: SECRET, now: NOW }),
      ),
      "invalid_payload",
    );
  });

  it("supports the documented duplicate-safe consumer: the same event id is acted on once", async () => {
    const seen = new Set<string>();
    let actions = 0;
    const handle = async () => {
      const e = await verifyWebhook({ payload: BODY, signature: HEADER, secret: SECRET, now: NOW });
      if (seen.has(e.id)) return "duplicate";
      seen.add(e.id);
      actions++;
      return "processed";
    };
    assert.deepEqual(
      [await handle(), await handle(), await handle()],
      ["processed", "duplicate", "duplicate"],
    );
    assert.equal(actions, 1);
  });
});

describe("client.webhooks", () => {
  const reply = (status: number, body?: unknown) =>
    new Response(body === undefined ? null : JSON.stringify(body), { status });
  function client(handler: (url: string, init: RequestInit) => Response) {
    const seen: { url: string; init: RequestInit }[] = [];
    const c = createClient({
      apiKey: "vk_test_synthetic_sdk_unit_0001",
      baseUrl: "http://api.invalid/",
      fetch: async (url, init) => {
        seen.push({ url: String(url), init: init ?? {} });
        return handler(String(url), init ?? {});
      },
    });
    return { c, seen };
  }

  it("manages endpoints and reads the delivery log through the documented routes", async () => {
    const endpoint = {
      id: "whe_1",
      url: "https://h.example.com/x",
      events: ["credits.low"],
      status: "enabled",
    };
    const delivery = { id: "whd_1", endpointId: "whe_1", eventId: "evt_1", status: "sent" };
    const detail = {
      ...delivery,
      event: { id: "evt_1", type: "x", createdAt: "t", data: {} },
      attemptLog: [],
    };
    const { c, seen } = client((u, init) => {
      if (init.method === "DELETE") return reply(204);
      if (u.includes("/webhook-deliveries/")) return reply(200, detail); // get and redeliver
      if (u.includes("/webhook-deliveries")) return reply(200, { data: [], nextCursor: null });
      if (u.endsWith("/test")) return reply(201, delivery);
      if (u.endsWith("/rotate-secret"))
        return reply(200, { ...endpoint, secret: "whsec_synthetic_x" });
      if (init.method === "GET" && u.endsWith("/webhook-endpoints"))
        return reply(200, { data: [endpoint] });
      if (init.method === "PATCH") return reply(200, { ...endpoint, status: "disabled" });
      return reply(201, { ...endpoint, secret: "whsec_synthetic_x" }); // create
    });
    const created = await c.webhooks.endpoints.create({
      url: "https://h.example.com/x",
      events: ["credits.low"],
    });
    assert.equal(created.secret, "whsec_synthetic_x");
    assert.equal((await c.webhooks.endpoints.list())[0]!.id, "whe_1");
    await c.webhooks.endpoints.update({ id: "whe_1", status: "disabled" });
    await c.webhooks.endpoints.rotateSecret({ id: "whe_1" });
    await c.webhooks.endpoints.test({ id: "whe_1" });
    await c.webhooks.endpoints.delete({ id: "whe_1" });
    await c.webhooks.deliveries.list({ endpointId: "whe_1", status: "failed", limit: 5 });
    await c.webhooks.deliveries.get({ id: "whd_1" });
    await c.webhooks.deliveries.redeliver({ id: "whd_1" });
    assert.deepEqual(
      seen.map((s) => `${s.init.method} ${s.url.replace("http://api.invalid", "")}`),
      [
        "POST /v1/webhook-endpoints",
        "GET /v1/webhook-endpoints",
        "PATCH /v1/webhook-endpoints/whe_1",
        "POST /v1/webhook-endpoints/whe_1/rotate-secret",
        "POST /v1/webhook-endpoints/whe_1/test",
        "DELETE /v1/webhook-endpoints/whe_1",
        "GET /v1/webhook-deliveries?endpointId=whe_1&status=failed&limit=5",
        "GET /v1/webhook-deliveries/whd_1",
        "POST /v1/webhook-deliveries/whd_1/redeliver",
      ],
    );
    assert.deepEqual(JSON.parse(String(seen[2]!.init.body)), { status: "disabled" });
  });
});
