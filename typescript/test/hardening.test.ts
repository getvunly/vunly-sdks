import assert from "node:assert/strict";
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";
import { describe, it } from "node:test";
import { Unavailable, VunlyError, createClient, getCustomerUsage } from "../src/index.ts";

const KEY = "vk_test_synthetic_sdk_hardening_0001";
// built from code points so no invisible character sits in this file
const ch = (cp: number) => String.fromCodePoint(cp);
const NUL = ch(0x00);
const ESC = ch(0x1b);
const BEL = ch(0x07);
const ZWSP = ch(0x200b);
const WJ = ch(0x2060);
const BOM = ch(0xfeff);
const RLO = ch(0x202e);
const LRI = ch(0x2066);

async function withServer<T>(
  handler: (req: IncomingMessage) => {
    status: number;
    headers?: Record<string, string>;
    body?: string;
  },
  run: (base: string, requests: IncomingMessage[]) => Promise<T>,
): Promise<T> {
  const requests: IncomingMessage[] = [];
  const server = createServer((req, res) => {
    requests.push(req);
    const r = handler(req);
    res.writeHead(r.status, { "content-type": "application/json", ...r.headers });
    res.end(r.body ?? "");
  });
  await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
  try {
    return await run(`http://127.0.0.1:${(server.address() as AddressInfo).port}`, requests);
  } finally {
    server.closeAllConnections();
    await new Promise((ok) => server.close(ok));
  }
}

const failing = async (p: Promise<unknown>): Promise<VunlyError> => {
  try {
    await p;
  } catch (e) {
    assert.ok(e instanceof VunlyError, String(e));
    return e;
  }
  throw new Error("expected the call to fail");
};

describe("redirects", () => {
  for (const status of [301, 302, 303, 307, 308]) {
    it(`a same-origin ${status} is unexpected_redirect: never followed, never retried, the key is sent once`, async () => {
      await withServer(
        () => ({ status, headers: { location: "/v1/customers" }, body: "{}" }),
        async (base, requests) => {
          const c = createClient({ apiKey: KEY, baseUrl: base, maxRetries: 3 });
          const e = await failing(c.customers.list());
          assert.ok(e instanceof Unavailable);
          assert.equal(e.code, "unexpected_redirect");
          assert.equal(e.status, status);
          await new Promise((r) => setTimeout(r, 100));
          assert.equal(requests.length, 1, "the redirect target must never be requested");
        },
      );
    });
  }

  it("a 3xx is unexpected_redirect whatever its body (even a well-formed API error)", async () => {
    await withServer(
      () => ({
        status: 302,
        headers: { location: "/x" },
        body: JSON.stringify({ error: { code: "not_found", message: "nope" } }),
      }),
      async (base, requests) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        assert.equal(e.code, "unexpected_redirect");
        assert.equal(requests.length, 1);
      },
    );
  });

  // What a browser's fetch returns for redirect: "manual": type "opaqueredirect", status 0 (not constructible with new Response).
  const opaque = () =>
    ({
      type: "opaqueredirect",
      status: 0,
      ok: false,
      redirected: false,
      headers: new Headers(),
      body: null,
    }) as unknown as Response;
  // A caller-supplied fetch that followed the redirect: a normal 200 with redirected: true.
  const followed = () => {
    const res = new Response(JSON.stringify({ customer: "cus_1" }), { status: 200 });
    Object.defineProperty(res, "redirected", { value: true });
    return res;
  };

  it("an opaque redirect (status 0) is unexpected_redirect and is called exactly once, even with retries", async () => {
    let calls = 0;
    const c = createClient({
      apiKey: KEY,
      baseUrl: "http://127.0.0.1:1",
      maxRetries: 3,
      fetch: async () => {
        calls++;
        return opaque();
      },
    });
    const e = await failing(c.customers.get({ customer: "cus_1" }));
    assert.ok(e instanceof Unavailable);
    assert.equal(e.code, "unexpected_redirect");
    assert.equal(calls, 1, "the key must be sent once");
  });

  it("getCustomerUsage: an opaque redirect is unexpected_redirect", async () => {
    const e = await failing(
      getCustomerUsage({
        baseUrl: "http://127.0.0.1:1",
        token: "ctok_synthetic_0001",
        fetch: async () => opaque(),
      }),
    );
    assert.equal(e.code, "unexpected_redirect");
  });

  it("a response a custom fetch already followed (redirected: true) is unexpected_redirect, called once", async () => {
    let calls = 0;
    const c = createClient({
      apiKey: KEY,
      baseUrl: "http://127.0.0.1:1",
      maxRetries: 3,
      fetch: async () => {
        calls++;
        return followed();
      },
    });
    const e = await failing(c.customers.get({ customer: "cus_1" }));
    assert.equal(e.code, "unexpected_redirect");
    assert.equal(calls, 1);
  });

  it("getCustomerUsage: a followed redirect (redirected: true) is unexpected_redirect", async () => {
    const e = await failing(
      getCustomerUsage({
        baseUrl: "http://127.0.0.1:1",
        token: "ctok_synthetic_0001",
        fetch: async () => followed(),
      }),
    );
    assert.equal(e.code, "unexpected_redirect");
  });

  it("getCustomerUsage refuses a redirect too and sends the token once", async () => {
    await withServer(
      () => ({ status: 302, headers: { location: "/v1/customer/usage" }, body: "{}" }),
      async (base, requests) => {
        const e = await failing(getCustomerUsage({ baseUrl: base, token: "ctok_synthetic_0001" }));
        assert.equal(e.code, "unexpected_redirect");
        await new Promise((r) => setTimeout(r, 100));
        assert.equal(requests.length, 1);
      },
    );
  });
});

describe("server text", () => {
  const reply = (error: unknown, headers: Record<string, string> = {}) =>
    ({ status: 400, headers, body: JSON.stringify({ error }) }) as const;

  it("strips control, BiDi and zero-width characters from message, code, request id and details", async () => {
    const dirty = `a${NUL}b${ESC}[31mc${RLO}d${LRI}e${ZWSP}f${WJ}g${BOM}h\nI`;
    await withServer(
      () =>
        reply({
          code: `bad${ZWSP}_code${RLO}`,
          message: dirty,
          requestId: `req${BEL}_${RLO}1`,
          details: { [`k${ZWSP}ey`]: [`x${RLO}y`, { n: `z${NUL}` }], num: 3 },
        }),
      async (base) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        assert.equal(e.message, "ab[31mcdefghI");
        assert.equal(e.code, "bad_code");
        assert.equal(e.requestId, "req_1");
        assert.deepEqual(e.details, { key: ["xy", { n: "z" }], num: 3 });
      },
    );
  });

  it("bounds message 500, code 64 and request id 128", async () => {
    await withServer(
      () =>
        reply({
          code: "c".repeat(300),
          message: "m".repeat(5000),
          requestId: "r".repeat(1000),
          details: { d: "d".repeat(5000) },
        }),
      async (base) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        assert.equal(e.message.length, 500);
        assert.equal(e.code.length, 64);
        assert.equal(e.requestId?.length, 128);
        assert.equal((e.details?.["d"] as string).length, 500);
      },
    );
  });

  it("details: exactly 50 entries per object or array, containers deeper than level 4 become null", async () => {
    const keys = Object.fromEntries(Array.from({ length: 51 }, (_, i) => [`k${i}`, i]));
    const nest = { a: { a: { a: { a: { a: "deep" } } } } };
    const list = (n: number) => Array.from({ length: n }, (_, i) => i);
    await withServer(
      () =>
        reply({
          code: "x",
          message: "m",
          details: { keys, list: list(51), exact: list(50), nest },
        }),
      async (base) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        const d = e.details as Record<string, any>;
        assert.equal(Object.keys(d["keys"]).length, 50);
        assert.ok("k49" in d["keys"] && !("k50" in d["keys"]));
        assert.equal(d["list"].length, 50);
        assert.equal(d["exact"].length, 50);
        // details is level 0, nest level 1: the container at level 5 is the first one past the bound
        assert.deepEqual(d["nest"], { a: { a: { a: { a: null } } } });
      },
    );
  });

  it("redacts an echoed key everywhere, also with an invisible character inside it or where the cut would split it", async () => {
    const split = `${KEY.slice(0, 10)}${ZWSP}${KEY.slice(10)}`;
    await withServer(
      () =>
        reply({
          code: `x_${KEY}`,
          message: `${"p".repeat(497)}${KEY} and ${split}`,
          requestId: `id-${KEY}`,
          details: { [KEY]: `Bearer ${KEY}`, list: [KEY], split: `s ${split} e` },
        }),
      async (base) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        const all = JSON.stringify([e.message, e.code, e.requestId, e.details]);
        assert.ok(!all.includes(KEY), all);
        assert.ok(!all.includes(KEY.slice(0, 20)), "no half of the key either");
        assert.ok(!e.message.includes("vk_"), "a cut must not leave the start of the key");
        assert.ok(e.requestId?.includes("<hidden>"));
        assert.equal(e.details?.["split"], "s <hidden> e", "a key split by a zero-width character");
        assert.ok(String(e.details?.["<hidden>"]).includes("<hidden>"));
      },
    );
  });

  it("redacts a key split by any Unicode format character (tags, Arabic number signs, musical, Brahmi, ...)", async () => {
    const splitters = [
      0xe0041, 0xe0001, 0xe007f, 0x600, 0x605, 0x6dd, 0x70f, 0x8e2, 0x110bd, 0x110cd, 0x1d173,
      0x1d17a,
    ];
    const details: Record<string, string> = {};
    for (const cp of splitters)
      details[`k${cp.toString(16)}`] = `a ${KEY.slice(0, 12)}${ch(cp)}${KEY.slice(12)} z`;
    await withServer(
      () => reply({ code: "x", message: "m", details }),
      async (base) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        for (const cp of splitters)
          assert.equal(
            e.details?.[`k${cp.toString(16)}`],
            "a <hidden> z",
            `split by U+${cp.toString(16).toUpperCase()}`,
          );
      },
    );
  });

  it("redacts the key from a request id that arrives with a malformed 2xx", async () => {
    await withServer(
      () => ({ status: 200, headers: { "request-id": `rid-${KEY}` }, body: "{}" }),
      async (base) => {
        const c = createClient({ apiKey: KEY, baseUrl: base });
        const e = await failing(c.customers.get({ customer: "cus_1" }));
        assert.equal(e.code, "invalid_response");
        assert.equal(e.requestId, "rid-<hidden>");
      },
    );
  });

  it("getCustomerUsage cleans and redacts the same way", async () => {
    const token = "ctok_synthetic_0002";
    await withServer(
      () => reply({ code: "x", message: `echo ${token}${RLO}`, requestId: `r${token}` }),
      async (base) => {
        const e = await failing(getCustomerUsage({ baseUrl: base, token }));
        assert.equal(e.message, "echo <hidden>");
        assert.equal(e.requestId, "r<hidden>");
      },
    );
  });
});
