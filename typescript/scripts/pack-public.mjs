// Builds the SDK to plain JavaScript with type declarations and packs a tarball that `npm install <file>` accepts.
// Nothing is published anywhere. Needs the development tools of this directory (`npm install` writes node_modules).
//   node scripts/pack-public.mjs [outDir]     (default: ./out)   prints the path of the tarball
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = resolve(process.argv[2] ?? join(pkgDir, "out"));
const stage = join(outDir, "stage");
const pkg = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8"));

rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });

execFileSync(
  process.execPath,
  [join(pkgDir, "node_modules/typescript/bin/tsc"), "-p", "tsconfig.build.json", "--outDir", stage],
  { cwd: pkgDir, stdio: "inherit" },
);

rmSync(join(stage, "version.d.ts"), { force: true });
// The verifier is a command (`npx vunly-verify`), not part of the library surface: ship its JavaScript, not its declarations.
for (const f of ["verify.d.ts", "verify-lib.d.ts", "cli.d.ts"])
  rmSync(join(stage, f), { force: true });

// `rewriteRelativeImportExtensions` fixes the .js output but leaves `./x.ts` in the .d.ts files; point declarations at `./x.js`.
for (const name of readdirSync(stage).filter((f) => f.endsWith(".d.ts"))) {
  const file = join(stage, name);
  const fixed = readFileSync(file, "utf8").replace(/(from\s+["']\.\/[^"']+)\.ts(["'])/g, "$1.js$2");
  if (/from\s+["']\.\/[^"']+\.ts["']/.test(fixed)) throw new Error(`${name}: a .ts import remains`);
  writeFileSync(file, fixed);
}

writeFileSync(
  join(stage, "package.json"),
  JSON.stringify(
    {
      name: pkg.name,
      version: pkg.version,
      description: pkg.description,
      type: "module",
      main: "index.js",
      types: "index.d.ts",
      exports: { ".": { types: "./index.d.ts", default: "./index.js" } },
      bin: { vunly: "cli.js", "vunly-verify": "verify.js" },
      files: ["*.js", "*.d.ts", "README.md", "LICENSE", "NOTICE"],
      engines: { node: ">=22" },
      sideEffects: false,
      license: pkg.license,
      repository: pkg.repository,
      homepage: pkg.homepage,
      bugs: pkg.bugs,
    },
    null,
    2,
  ) + "\n",
);
for (const f of ["README.md", "LICENSE", "NOTICE"]) cpSync(join(pkgDir, f), join(stage, f));

const out = execFileSync("npm", ["pack", "--pack-destination", outDir, "--silent"], {
  cwd: stage,
  encoding: "utf8",
}).trim();
const tarball = join(outDir, out.split("\n").at(-1));
rmSync(stage, { recursive: true, force: true });
console.log(tarball);
