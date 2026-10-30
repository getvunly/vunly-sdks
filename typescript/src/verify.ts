#!/usr/bin/env node
// `vunly-verify`: the same as `vunly verify` (kept so that `npx vunly-verify` keeps working).
//
//   export VUNLY_API_URL=...      the sandbox API address given in the setup step (with or without /v1)
//   export VUNLY_API_KEY=...      your sandbox key (vk_test_...); never written to the report
//   npx vunly-verify --meter <your-meter-key>
//
// --meter is optional when the project has exactly one meter. Exit code: 0 every check passed, 1 a check failed,
// 2 the verifier could not run (setup problem).
/// <reference types="node" />
import { SetupProblem, runVerify } from "./verify-lib.ts";
import { VERSION } from "./version.ts";

const USAGE = `Usage: npx vunly-verify [--meter <key>]

Runs the 5 sandbox checks against your project and writes vunly-verify-report.md.

  export VUNLY_API_URL=<sandbox API address from the setup step>
  export VUNLY_API_KEY=<sandbox key, vk_test_...>   (read from the environment only, never written to the report)

  --meter <key>   the meter to check; optional when the project has exactly one
  --help, -h      show this text
  --version, -v   show the version

Exit code: 0 every check passed, 1 a check failed, 2 the verifier could not run (setup problem).
Also available as \`npx vunly verify\`.`;
const first = process.argv[2];
if (first === "--help" || first === "-h" || first === "help") {
  console.log(USAGE);
  process.exit(0);
}
if (first === "--version" || first === "-v") {
  console.log(`vunly-verify ${VERSION}`);
  process.exit(0);
}

try {
  process.exit(await runVerify(process.argv.slice(2)));
} catch (e) {
  if (!(e instanceof SetupProblem)) throw e;
  console.error(`vunly-verify: cannot run: ${e.message}`);
  process.exit(2);
}
