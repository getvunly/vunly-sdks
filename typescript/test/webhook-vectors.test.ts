import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { WebhookSignatureError, verifyWebhook } from "../src/index.ts";

// The shared webhook vectors (webhook-vectors.json, an identical copy in every SDK). Signatures were computed outside any
// SDK. "expect" is "ok", a WebhookSignatureError reason, or "config" (a plain Error raised before the signature is looked at).
interface Vector {
  name: string;
  payload: string;
  header: string | null;
  secrets: string[];
  expect: string;
}
const file = JSON.parse(
  readFileSync(new URL("./webhook-vectors.json", import.meta.url), "utf8"),
) as { now: number; vectors: Vector[] };

describe("shared webhook vectors", () => {
  it("there are enough vectors", () => {
    assert.ok(file.vectors.length >= 20);
  });

  for (const v of file.vectors) {
    it(`${v.name} -> ${v.expect}`, async () => {
      let got = "ok";
      try {
        await verifyWebhook({
          payload: v.payload,
          signature: v.header,
          secret: v.secrets,
          now: file.now * 1000,
        });
      } catch (e) {
        if (e instanceof WebhookSignatureError) got = e.reason;
        else if (e instanceof Error && /secret is empty or missing/.test(e.message)) got = "config";
        else throw e;
      }
      assert.equal(got, v.expect);
    });
  }
});
