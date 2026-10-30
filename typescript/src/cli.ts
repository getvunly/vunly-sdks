#!/usr/bin/env node
/// <reference types="node" />
// The `vunly` command: `vunly init`, `vunly doctor`, `vunly verify`. The same three commands exist for Python projects
// (`pip install vunly`, then `vunly ...`).
//
//   export VUNLY_API_URL=...      the API address from the setup step (with or without /v1)
//   export VUNLY_API_KEY=...      your sandbox key (vk_test_...); read from the environment ONLY
//
// The key is never printed, never written to a file and never accepted as an argument.
// Exit codes: 0 everything passed, 1 a check failed, 2 the command could not run (a setup problem).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { normalizeBaseUrl } from "./index.ts";
import { SetupProblem, isRedirectResponse, runVerify } from "./verify-lib.ts";

import { VERSION } from "./version.ts";
const HELP = `vunly ${VERSION}: sandbox tools
  vunly init      write .env.example (names only) and ignore .env; never writes a key
  vunly doctor    check Node, the environment, the API address, the key and its scopes
  vunly verify    run the 5 sandbox checks (--meter <key> if the project has several); writes vunly-verify-report.md
Exit codes: 0 ok, 1 a check failed, 2 could not run.`;

async function doctor(): Promise<number> {
  const rows: [string, string, string][] = [];
  const add = (status: "PASS" | "WARN" | "FAIL", name: string, detail: string) =>
    rows.push([status, name, detail]);
  const major = Number(process.versions.node.split(".")[0]);
  add(
    major >= 22 ? "PASS" : "FAIL",
    "Node",
    `${process.versions.node} (needs 22 or newer); @vunly/sdk ${VERSION}`,
  );

  const rawUrl = process.env["VUNLY_API_URL"] ?? "";
  const key = process.env["VUNLY_API_KEY"] ?? "";
  if (!rawUrl) add("FAIL", "VUNLY_API_URL", "not set: copy the API address from the setup step");
  if (!key)
    add(
      "FAIL",
      "VUNLY_API_KEY",
      "not set: copy your sandbox key from the setup step (never write it into a file)",
    );
  else if (!key.startsWith("vk_test_"))
    add(
      "FAIL",
      "VUNLY_API_KEY",
      "is set but is not a sandbox key (vk_test_...): this tool only runs with a sandbox key",
    );
  else add("PASS", "VUNLY_API_KEY", "set, sandbox key (value not shown)");

  if (rawUrl) {
    const base = normalizeBaseUrl(rawUrl);
    const host = (() => {
      try {
        return new URL(base).host;
      } catch {
        return "(invalid url)";
      }
    })();
    const note = /\/v1\/*$/i.test(rawUrl.trim())
      ? " (the trailing /v1 is accepted and ignored)"
      : "";
    add("PASS", "VUNLY_API_URL", `${host}${note}`);
    let reachable = false;
    try {
      // redirect: "manual": a redirect is reported, never followed
      const health = await fetch(`${base}/v1/health`, {
        redirect: "manual",
        signal: AbortSignal.timeout(10_000),
      });
      const redirected = isRedirectResponse(health);
      reachable = health.ok && !redirected;
      add(
        reachable ? "PASS" : "FAIL",
        "API reachable",
        redirected
          ? `GET /v1/health answered a redirect${health.status >= 300 && health.status < 400 ? ` (HTTP ${health.status})` : ""}, which is never followed: VUNLY_API_URL is probably not the API address; use the address from the setup step`
          : health.status === 404
            ? "GET /v1/health answered HTTP 404 (no such route): VUNLY_API_URL is probably not the API address; use the address from the setup step (a trailing /v1 is accepted)"
            : `GET /v1/health answered HTTP ${health.status}`,
      );
      const date = health.headers.get("date");
      if (date) {
        const skew = Math.abs(Date.now() - Date.parse(date)) / 1000;
        add(
          skew < 60 ? "PASS" : "WARN",
          "Clock",
          `${skew.toFixed(0)} s from the API's clock${skew < 60 ? "" : " (token lifetimes depend on it)"}`,
        );
      }
    } catch (e) {
      add(
        "FAIL",
        "API reachable",
        `cannot reach ${host} (${(e as Error).name}); check VUNLY_API_URL and the network`,
      );
    }
    if (reachable && key.startsWith("vk_test_")) {
      for (const [scope, path] of [
        ["plans:read", "/v1/meters"],
        ["customers:read", "/v1/customers?limit=1"],
      ] as const) {
        let r: Response;
        try {
          r = await fetch(`${base}${path}`, {
            headers: { authorization: `Bearer ${key}` },
            redirect: "manual",
            signal: AbortSignal.timeout(10_000),
          });
        } catch {
          continue;
        }
        if (isRedirectResponse(r)) {
          add(
            "FAIL",
            `Key request (${scope})`,
            `GET ${path} answered a redirect${r.status >= 300 && r.status < 400 ? ` (HTTP ${r.status})` : ""}, which is never followed (the key is not sent to another address): check VUNLY_API_URL`,
          );
          break;
        }
        if (r.status === 401)
          add(
            "FAIL",
            `Key accepted (${scope})`,
            "the API does not know this key (401): use the sandbox key from the setup step",
          );
        else if (r.status === 403)
          add("WARN", `Scope ${scope}`, "the key lacks it (403): `vunly verify` needs it");
        else if (r.status === 404 && path === "/v1/meters")
          add(
            "FAIL",
            "API address",
            "answered 404 (no such route): VUNLY_API_URL is probably not the API address",
          );
        else if (r.ok) {
          add("PASS", `Scope ${scope}`, "ok");
          if (path === "/v1/meters") {
            // a malformed 2xx (non-JSON, or no `data` array) is a FAIL row, never a stack trace
            let n: number | null = null;
            try {
              const body = (await r.json()) as { data?: unknown[] };
              if (Array.isArray(body.data)) n = body.data.length;
            } catch {
              n = null;
            }
            if (n === null)
              add(
                "FAIL",
                "Meters",
                "the API answered /v1/meters with a body this tool did not understand",
              );
            else
              add(
                n ? "PASS" : "WARN",
                "Meters",
                `${n} configured${n ? "" : ": create one (or apply a recipe) before `vunly verify`"}`,
              );
          }
        }
      }
    }
  }
  const width = Math.max(...rows.map((r) => r[1].length));
  for (const [s, n, d] of rows) console.log(`${s.padEnd(4)} ${n.padEnd(width)}  ${d}`);
  const bad = rows.filter((r) => r[0] === "FAIL").length;
  const warn = rows.filter((r) => r[0] === "WARN").length;
  console.log(`\nResult: ${rows.length - bad - warn} ok, ${warn} warnings, ${bad} problems.`);
  return bad > 0 ? 1 : 0;
}

function init(): number {
  const made: string[] = [];
  if (!existsSync(".env.example")) {
    writeFileSync(
      ".env.example",
      "# Copy the values from the Vunly setup step into your SHELL (export ...) or your secret manager.\n" +
        "# Do not put the real key in this file or in any file you commit.\nVUNLY_API_URL=\nVUNLY_API_KEY=\n",
    );
    made.push(".env.example (names only, no values)");
  }
  const text = existsSync(".gitignore") ? readFileSync(".gitignore", "utf8") : "";
  // `node_modules/` and `/node_modules` are the same entry
  const norm = (s: string) => s.replace(/^\//, "").replace(/\/$/, "");
  const lines = text.split(/\s+/).map(norm);
  const hasNext = ["js", "mjs", "cjs", "ts"].some((e) => existsSync(`next.config.${e}`));
  // `.env.*` would also hide `.env.example`, which you commit: the `!` line (after it) puts it back.
  // Dependencies and build output are never committed either, and `git add -A` would stage them.
  const wanted = [
    ".env",
    ".env.*",
    "!.env.example",
    "node_modules/",
    ...(hasNext ? [".next/"] : []),
  ].filter((p) => !lines.includes(norm(p)));
  if (wanted.length > 0) {
    writeFileSync(
      ".gitignore",
      text + (text === "" || text.endsWith("\n") ? "" : "\n") + wanted.join("\n") + "\n",
    );
    made.push(
      `.gitignore (added ${wanted.join(", ")}; .env is ignored, .env.example stays committable)`,
    );
  }
  console.log(
    `vunly init: ${made.length > 0 ? "wrote " + made.join("; ") : "nothing to change, already set up"}`,
  );
  console.log(
    "\nNext:\n" +
      "  1. In your shell:  export VUNLY_API_URL=<address from the setup step>  and  export VUNLY_API_KEY=<sandbox key>\n" +
      "  2. npx vunly doctor    checks the environment, the address, the key and its scopes\n" +
      "  3. npx vunly verify    runs the 5 sandbox checks and writes vunly-verify-report.md\n" +
      "The key is read from the environment only: it is never written to a file by this command.",
  );
  return 0;
}

const [command, ...rest] = process.argv.slice(2);
try {
  if (command === "verify") process.exit(await runVerify(rest));
  else if (command === "doctor") process.exit(await doctor());
  else if (command === "init") process.exit(init());
  else if (command === "--version" || command === "-v") {
    console.log(`vunly ${VERSION}`);
    process.exit(0);
  } else {
    console.log(HELP);
    process.exit(command === undefined || command === "--help" || command === "help" ? 0 : 2);
  }
} catch (e) {
  if (!(e instanceof SetupProblem)) throw e;
  console.error(`vunly ${command}: cannot run: ${e.message}`);
  process.exit(2);
}
