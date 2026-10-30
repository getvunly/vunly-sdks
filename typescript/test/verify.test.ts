// `vunly verify` / `npx vunly-verify` must treat a malformed GET /v1/meters as a setup problem (exit 2), never a
// raw SyntaxError (non-JSON) or TypeError (no `data`, or `data` not an array). runVerify throws SetupProblem, which
// the CLI turns into exit 2. These fail without the findMeter shape check.
import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";
import { runVerify, SetupProblem } from "../src/verify-lib.ts";

const realFetch = globalThis.fetch;
const realUrl = process.env.VUNLY_API_URL;
const realKey = process.env.VUNLY_API_KEY;

describe("vunly verify: malformed /v1/meters", () => {
  beforeEach(() => {
    process.env.VUNLY_API_URL = "http://api.invalid";
    process.env.VUNLY_API_KEY = "vk_test_synthetic_verify_0001";
  });
  afterEach(() => {
    globalThis.fetch = realFetch;
    if (realUrl === undefined) delete process.env.VUNLY_API_URL;
    else process.env.VUNLY_API_URL = realUrl;
    if (realKey === undefined) delete process.env.VUNLY_API_KEY;
    else process.env.VUNLY_API_KEY = realKey;
  });

  const metersReturns = (make: () => Response) => {
    globalThis.fetch = (async (input: Parameters<typeof fetch>[0]) => {
      if (String(input).includes("/v1/meters")) return make();
      throw new Error("unexpected request");
    }) as typeof fetch;
  };

  for (const [label, make] of [
    ["non-JSON", () => new Response("<html>proxy</html>", { status: 200 })],
    ["empty object", () => new Response(JSON.stringify({}), { status: 200 })],
    [
      "null",
      () => new Response("null", { status: 200, headers: { "content-type": "application/json" } }),
    ],
    ["array", () => new Response(JSON.stringify([]), { status: 200 })],
    ["data not an array", () => new Response(JSON.stringify({ data: 5 }), { status: 200 })],
    ["data of non-objects", () => new Response(JSON.stringify({ data: [1, 2] }), { status: 200 })],
  ] as const) {
    it(`${label} is a SetupProblem, not a crash`, async () => {
      metersReturns(make);
      // a SetupProblem (-> exit 2) is the handled path; a raw TypeError/SyntaxError would NOT be a SetupProblem
      await assert.rejects(
        runVerify(["--meter", "tokens"]),
        (e: Error) => e instanceof SetupProblem,
        label,
      );
    });
  }
});
