// `vunly doctor` and `vunly verify` never follow a redirect and never send the key to a redirect target: a 3xx is
// reported as an error. Each test runs the real command against a local server whose answer redirects to a second
// ("target") server, then asserts the target saw nothing.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { describe, it } from "node:test";
import { SetupProblem, runVerify } from "../src/verify-lib.ts";

const KEY = "vk_test_synthetic_cli_redirect_0001";
const CLI = new URL("../src/cli.ts", import.meta.url).pathname;

const listen = async (
  handler: (req: IncomingMessage) => {
    status: number;
    headers?: Record<string, string>;
    body?: string;
  },
  seen: IncomingMessage[],
): Promise<{ server: Server; base: string }> => {
  const server = createServer((req, res) => {
    seen.push(req);
    const r = handler(req);
    res.writeHead(r.status, { "content-type": "application/json", ...r.headers });
    res.end(r.body ?? "{}");
  });
  await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
  return { server, base: `http://127.0.0.1:${(server.address() as AddressInfo).port}` };
};
const close = (s: Server) =>
  new Promise<void>((ok) => {
    s.closeAllConnections();
    s.close(() => ok());
  });

const run = (command: string, base: string) =>
  new Promise<{ code: number; out: string }>((done) => {
    execFile(
      process.execPath,
      ["--no-warnings", CLI, command],
      {
        env: { PATH: process.env["PATH"] ?? "", VUNLY_API_URL: base, VUNLY_API_KEY: KEY },
        cwd: "/tmp",
      },
      (err, stdout, stderr) =>
        done({
          code: err ? Number((err as unknown as { code: unknown }).code) : 0,
          out: stdout + stderr,
        }),
    );
  });

/** A "front" server answering `front`, and a "target" it redirects to; returns what each saw. */
async function withRedirect<T>(
  front: (
    req: IncomingMessage,
    targetBase: string,
  ) => { status: number; headers?: Record<string, string>; body?: string },
  body: (base: string, frontSeen: IncomingMessage[], targetSeen: IncomingMessage[]) => Promise<T>,
): Promise<T> {
  const targetSeen: IncomingMessage[] = [];
  const frontSeen: IncomingMessage[] = [];
  const target = await listen(
    () => ({ status: 200, body: JSON.stringify({ data: [] }) }),
    targetSeen,
  );
  const f = await listen((req) => front(req, target.base), frontSeen);
  try {
    return await body(f.base, frontSeen, targetSeen);
  } finally {
    await close(f.server);
    await close(target.server);
  }
}

describe("vunly doctor: a redirect is reported, never followed", () => {
  it("a redirected /v1/health is a FAIL row and the target is never requested", async () => {
    await withRedirect(
      (_req, t) => ({ status: 302, headers: { location: `${t}/v1/health` } }),
      async (base, frontSeen, targetSeen) => {
        const r = await run("doctor", base);
        assert.equal(r.code, 1, r.out);
        assert.match(
          r.out,
          /FAIL\s+API reachable\s+GET \/v1\/health answered a redirect \(HTTP 302\)/,
        );
        assert.equal(targetSeen.length, 0, "the redirect target must never be requested");
        assert.equal(frontSeen.length, 1);
        assert.ok(!r.out.includes(KEY));
      },
    );
  });

  it("a redirected key request is a FAIL row; the key never reaches the target", async () => {
    await withRedirect(
      (req, t) =>
        req.url === "/v1/health"
          ? { status: 200 }
          : { status: 307, headers: { location: `${t}${req.url}` } },
      async (base, frontSeen, targetSeen) => {
        const r = await run("doctor", base);
        assert.equal(r.code, 1, r.out);
        assert.match(
          r.out,
          /FAIL\s+Key request \(plans:read\)\s+GET \/v1\/meters answered a redirect \(HTTP 307\)/,
        );
        assert.equal(targetSeen.length, 0, "the key must never be sent to the redirect target");
        assert.equal(
          frontSeen.filter((q) => q.headers.authorization).length,
          1,
          "the key is sent once",
        );
        assert.ok(!r.out.includes(KEY));
      },
    );
  });
});

// An answer a custom fetch already followed: status 200, no 3xx, not an opaque redirect, only `redirected: true`. It
// is the clause of isRedirectResponse that no real server can trigger, so the global fetch is replaced.
describe("an answer a fetch already followed (status 200, redirected: true) is a redirect failure", () => {
  const PRELOAD = new URL("./fixtures/followed-redirect-fetch.mjs", import.meta.url).pathname;
  const doctorWith = (on: "health" | "meters") =>
    new Promise<{ code: number; out: string }>((done) => {
      execFile(
        process.execPath,
        ["--no-warnings", "--import", PRELOAD, CLI, "doctor"],
        {
          env: {
            PATH: process.env["PATH"] ?? "",
            VUNLY_API_URL: "http://api.invalid",
            VUNLY_API_KEY: KEY,
            VUNLY_TEST_REDIRECTED_ON: on,
          },
          cwd: "/tmp",
        },
        (err, stdout, stderr) =>
          done({
            code: err ? Number((err as unknown as { code: unknown }).code) : 0,
            out: stdout + stderr,
          }),
      );
    });

  it("doctor: a followed /v1/health is a FAIL row, not a pass", async () => {
    const r = await doctorWith("health");
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /FAIL\s+API reachable\s+GET \/v1\/health answered a redirect/);
    assert.ok(!r.out.includes(KEY));
  });

  it("doctor: a followed key request is a FAIL row", async () => {
    const r = await doctorWith("meters");
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /FAIL\s+Key request \(plans:read\)\s+GET \/v1\/meters answered a redirect/);
    assert.ok(!r.out.includes(KEY));
  });

  it("verify: a followed /v1/meters is a SetupProblem and the key is not in the message", async () => {
    const real = globalThis.fetch;
    const realUrl = process.env["VUNLY_API_URL"];
    const realKey = process.env["VUNLY_API_KEY"];
    process.env["VUNLY_API_URL"] = "http://api.invalid";
    process.env["VUNLY_API_KEY"] = KEY;
    globalThis.fetch = (async () => {
      const res = new Response(JSON.stringify({ data: [] }), { status: 200 });
      Object.defineProperty(res, "redirected", { value: true });
      return res;
    }) as typeof fetch;
    try {
      await assert.rejects(
        runVerify(["--meter", "tokens"]),
        (e: Error) =>
          e instanceof SetupProblem && /redirect/.test(e.message) && !e.message.includes(KEY),
      );
    } finally {
      globalThis.fetch = real;
      if (realUrl === undefined) delete process.env["VUNLY_API_URL"];
      else process.env["VUNLY_API_URL"] = realUrl;
      if (realKey === undefined) delete process.env["VUNLY_API_KEY"];
      else process.env["VUNLY_API_KEY"] = realKey;
    }
  });
});

describe("vunly verify: a redirected /v1/meters is a setup problem, never followed", () => {
  it("exits 2 with a redirect message and the target is never requested", async () => {
    await withRedirect(
      (req, t) => ({ status: 301, headers: { location: `${t}${req.url}` } }),
      async (base, frontSeen, targetSeen) => {
        const r = await run("verify", base);
        assert.equal(r.code, 2, r.out);
        assert.match(
          r.out,
          /GET \/v1\/meters answered a redirect \(HTTP 301\), which is never followed/,
        );
        assert.equal(targetSeen.length, 0, "the key must never be sent to the redirect target");
        assert.equal(frontSeen.length, 1);
        assert.ok(!r.out.includes(KEY));
      },
    );
  });

  it("an opaque redirect (status 0, as a browser fetch returns) is a SetupProblem too", async () => {
    const real = globalThis.fetch;
    const realUrl = process.env["VUNLY_API_URL"];
    const realKey = process.env["VUNLY_API_KEY"];
    process.env["VUNLY_API_URL"] = "http://api.invalid";
    process.env["VUNLY_API_KEY"] = KEY;
    globalThis.fetch = (async () =>
      ({
        type: "opaqueredirect",
        status: 0,
        ok: false,
        redirected: false,
        headers: new Headers(),
        body: null,
      }) as unknown as Response) as typeof fetch;
    try {
      await assert.rejects(
        runVerify(["--meter", "tokens"]),
        (e: Error) => e instanceof SetupProblem && /redirect/.test(e.message),
      );
    } finally {
      globalThis.fetch = real;
      if (realUrl === undefined) delete process.env["VUNLY_API_URL"];
      else process.env["VUNLY_API_URL"] = realUrl;
      if (realKey === undefined) delete process.env["VUNLY_API_KEY"];
      else process.env["VUNLY_API_KEY"] = realKey;
    }
  });
});
