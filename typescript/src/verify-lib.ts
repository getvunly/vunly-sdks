/// <reference types="node" />
// The sandbox verifier behind `vunly verify` and `vunly-verify`: checks an integration end to end using ONLY the
// installed SDK and your sandbox key. It creates one throwaway customer, tops it up with sandbox credits and records
// usage on it. No real payment is made and nothing is deployed. A report without secrets is written to
// ./vunly-verify-report.md. Needs the scopes the setup step issues (customers, plans:read, credits:grant, usage:write,
// balance:read). Returns the exit code: 0 every check passed, 1 a check failed; throws SetupProblem when it cannot run.
import { writeFileSync } from "node:fs";
import {
  AuthError,
  LimitReached,
  Unavailable,
  VunlyError,
  createClient,
  normalizeBaseUrl,
} from "./index.ts";

/** The command cannot run: say what to fix (exit code 2). */
export class SetupProblem extends Error {}

/** The API address and sandbox key from the environment, or a SetupProblem that says what is missing. */
export function readEnv(): { baseUrl: string; apiKey: string } {
  const apiKey = process.env["VUNLY_API_KEY"] ?? "";
  // with or without a trailing slash or /v1: the same address
  const baseUrl = normalizeBaseUrl(process.env["VUNLY_API_URL"] ?? "");
  if (!baseUrl) throw new SetupProblem("VUNLY_API_URL is not set (copy it from the setup step).");
  if (!apiKey) throw new SetupProblem("VUNLY_API_KEY is not set (copy it from the setup step).");
  if (!apiKey.startsWith("vk_test_"))
    throw new SetupProblem(
      "this verifier only runs with a sandbox key (vk_test_...). No other key is accepted.",
    );
  return { baseUrl, apiKey };
}

/** A 3xx, a browser's opaque redirect (status 0) or an answer a fetch already followed: never followed by these tools. */
export function isRedirectResponse(res: Response): boolean {
  return (
    res.type === "opaqueredirect" ||
    res.redirected === true ||
    (res.status >= 300 && res.status < 400)
  );
}

async function findMeter(
  baseUrl: string,
  apiKey: string,
  wanted: string | undefined,
): Promise<{ key: string; unitsPerCredit: number }> {
  let res: Response;
  try {
    // redirect: "manual": a redirect is never followed, so the key is never sent to another address
    res = await fetch(`${baseUrl}/v1/meters`, {
      headers: { authorization: `Bearer ${apiKey}` },
      redirect: "manual",
    });
  } catch (e) {
    throw new SetupProblem(`cannot reach ${baseUrl} (${(e as Error).message}).`);
  }
  if (isRedirectResponse(res)) {
    void res.body?.cancel().catch(() => {});
    throw new SetupProblem(
      `GET /v1/meters answered a redirect${res.status >= 300 && res.status < 400 ? ` (HTTP ${res.status})` : ""}, which is never followed. VUNLY_API_URL is probably not the API address: use the address from the setup step.`,
    );
  }
  if (res.status === 401 || res.status === 403)
    throw new SetupProblem(
      `the API refused the key (HTTP ${res.status}). Use the sandbox key from the setup step.`,
    );
  if (res.status === 404)
    throw new SetupProblem(
      `GET /v1/meters answered HTTP 404 (no such route). VUNLY_API_URL is probably not the API address: use the address from the setup step, for example https://host (a trailing /v1 is accepted). Tried ${baseUrl}/v1/meters.`,
    );
  if (!res.ok) throw new SetupProblem(`GET /v1/meters answered HTTP ${res.status}.`);
  // A malformed 2xx (non-JSON, no `data`, or `data` not an array of meters) is a setup problem (exit 2), not a crash.
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new SetupProblem("GET /v1/meters did not return JSON this tool understands.");
  }
  const meters = (body as { data?: unknown } | null)?.data;
  if (
    !Array.isArray(meters) ||
    !meters.every(
      (m): m is { key: string; unitsPerCredit: number } =>
        typeof m === "object" && m !== null && typeof (m as { key?: unknown }).key === "string",
    )
  )
    throw new SetupProblem("GET /v1/meters returned a body this tool did not understand.");
  const found = wanted
    ? meters.find((m) => m.key === wanted)
    : meters.length === 1
      ? meters[0]
      : undefined;
  if (!found)
    throw new SetupProblem(
      wanted
        ? `the project has no meter "${wanted}" (it has: ${meters.map((m) => m.key).join(", ") || "none"}).`
        : `pass --meter <key>; the project has ${meters.length === 0 ? "no meter yet" : `these meters: ${meters.map((m) => m.key).join(", ")}`}.`,
    );
  return found;
}

export async function runVerify(argv: string[]): Promise<number> {
  const { baseUrl, apiKey } = readEnv();
  const i = argv.indexOf("--meter");
  const meter = await findMeter(baseUrl, apiKey, i >= 0 ? argv[i + 1] : undefined);

  const vunly = createClient({ apiKey, baseUrl });
  type Result = { id: string; name: string; ok: boolean; detail: string };
  const results: Result[] = [];
  async function check(id: string, name: string, fn: () => Promise<string>): Promise<void> {
    try {
      results.push({ id, name, ok: true, detail: await fn() });
    } catch (e) {
      const hint =
        e instanceof AuthError
          ? " (the key was refused or lacks a scope: use the key from the setup step)"
          : "";
      results.push({ id, name, ok: false, detail: `${(e as Error).message}${hint}` });
    }
  }
  function ok(cond: unknown, msg: string): asserts cond {
    if (!cond) throw new Error(msg);
  }

  const run = Math.random().toString(36).slice(2, 10);
  const customer = `vunly-verify-${run}`;
  const per = meter.unitsPerCredit;
  const GRANT = 5;
  const balance = async (): Promise<number> =>
    (await vunly.credits.balance({ customer })).remaining;

  await check("V1", "Customers are created idempotently", async () => {
    await vunly.customers.upsert({ id: customer });
    await vunly.customers.upsert({ id: customer });
    ok((await balance()) === 0, "a new customer should start at 0 credits");
    return `customer ${customer} created twice, one customer, balance 0`;
  });

  await check("V2", "A top-up with the same id is applied once", async () => {
    const id = `verify-grant-${run}`;
    await vunly.credits.grant({ id, customer, credits: GRANT });
    await vunly.credits.grant({ id, customer, credits: GRANT });
    const b = await balance();
    ok(b === GRANT, `balance ${b} after granting twice with one id, expected ${GRANT}`);
    return `granted ${GRANT} credits twice with one id: balance ${b}`;
  });

  await check("V3", "Allowed usage is counted once; a replay does not double count", async () => {
    const id = `verify-use-${run}`;
    const first = await vunly.usage.record({ id, customer, meter: meter.key, quantity: per });
    const again = await vunly.usage.record({ id, customer, meter: meter.key, quantity: per });
    const b = await balance();
    ok(b === GRANT - 1, `balance ${b} after recording the same id twice, expected ${GRANT - 1}`);
    ok(
      again.replayed === true && first.replayed === false,
      "the second record was not reported as a replay",
    );
    return `1 credit (${per} ${meter.key}) recorded twice with one id: balance ${b}, second call reported as replay`;
  });

  await check("V4", "Over-limit usage is rejected and nothing is counted", async () => {
    const before = await balance();
    const tooMuch = (before + 1) * per;
    let refused = false;
    try {
      await vunly.usage.record({
        id: `verify-over-${run}`,
        customer,
        meter: meter.key,
        quantity: tooMuch,
      });
    } catch (e) {
      refused = e instanceof LimitReached;
      if (!refused) throw e;
    }
    ok(refused, "usage above the balance was accepted");
    ok((await balance()) === before, "a refused record changed the balance");
    return `${tooMuch} ${meter.key} against ${before} credits: LimitReached (402), balance still ${before}`;
  });

  await check("V5", "An unreachable Vunly fails closed", async () => {
    const down = createClient({ apiKey, baseUrl: "http://127.0.0.1:9", maxRetries: 0 });
    try {
      await down.entitlements.check({
        customer,
        feature: "any",
        estimate: { meter: meter.key, quantity: 1 },
      });
    } catch (e) {
      ok(
        e instanceof Unavailable,
        `an unreachable API threw ${(e as Error).name}, expected Unavailable`,
      );
      return "with the API unreachable, entitlements.check throws Unavailable: your code must not run the work";
    }
    throw new Error("an unreachable API did not throw");
  });

  const failed = results.filter((r) => !r.ok);
  for (const r of results)
    console.log(`${r.ok ? "PASS" : "FAIL"} ${r.id} ${r.name}\n     ${r.detail}`);
  const summary = `Summary: ${results.map((r) => `${r.id} ${r.ok ? "PASS" : "FAIL"}`).join(", ")}`;
  const result = `Result: ${results.length - failed.length} of ${results.length} checks passed. Sandbox only; no real payments were made.`;
  console.log(`\n${summary}\n${result}`);
  const host = (() => {
    try {
      return new URL(baseUrl).host;
    } catch {
      return "(invalid url)";
    }
  })();
  writeFileSync(
    "vunly-verify-report.md",
    [
      "# Vunly sandbox verification",
      "",
      `API: ${host} · meter: ${meter.key} · customer: ${customer} · key: sandbox (not recorded)`,
      "",
      ...results.map((r) => `- **${r.ok ? "PASS" : "FAIL"} ${r.id}** ${r.name}: ${r.detail}`),
      "",
      result,
      "",
    ].join("\n"),
  );
  return failed.length > 0 ? 1 : 0;
}

// keep VunlyError referenced for callers that catch it
export { VunlyError };
