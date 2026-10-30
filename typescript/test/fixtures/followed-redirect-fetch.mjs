// Preloaded into `vunly doctor` by cli-redirect.test.ts (node --import). It replaces global fetch with one that
// "already followed a redirect": a plain 200 whose `redirected` is true (not a 3xx, not an opaque redirect), like a
// custom fetch that ignores redirect: "manual". VUNLY_TEST_REDIRECTED_ON picks the path that answers so: "health" or
// "meters"; any other path answers a normal 200.
const on = process.env.VUNLY_TEST_REDIRECTED_ON;
globalThis.fetch = async (input) => {
  const url = String(input);
  const res = new Response(JSON.stringify({ data: [] }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
  if (url.includes(on === "health" ? "/v1/health" : "/v1/meters"))
    Object.defineProperty(res, "redirected", { value: true });
  return res;
};
