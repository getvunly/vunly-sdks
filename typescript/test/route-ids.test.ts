import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { InvalidRequest, createClient } from "../src/index.ts";

const KEY = "vk_test_synthetic_sdk_route_ids_0001";

// An empty, "." or ".." id would change the route on the wire (GET /v1/customers/, GET /v1/): refused before any request.
describe("an id that would change the route", () => {
  for (const bad of ["", ".", ".."]) {
    it(`${JSON.stringify(bad)} is refused before any request`, async () => {
      let hits = 0;
      const client = createClient({
        apiKey: KEY,
        baseUrl: "http://api.invalid",
        maxRetries: 0,
        fetch: async () => {
          hits += 1;
          return new Response("{}", {
            status: 200,
            headers: { "content-type": "application/json" },
          });
        },
      });
      await assert.rejects(
        () => client.customers.get({ customer: bad }),
        (e: unknown) => e instanceof InvalidRequest && JSON.stringify(e.details).includes("fields"),
      );
      await assert.rejects(() => client.credits.balance({ customer: bad }), InvalidRequest);
      assert.equal(hits, 0);
    });
  }
});
