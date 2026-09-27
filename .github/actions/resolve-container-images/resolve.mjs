import { createRequire } from "node:module";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";

const require = createRequire(import.meta.url);
const {
  loadCatalog,
  resolveImages,
} = require("../../../ci/container-images.cjs");

try {
  const workspace = process.env.GITHUB_WORKSPACE;
  const outputFile = process.env.GITHUB_OUTPUT;
  if (!workspace || !outputFile) throw new Error("GitHub Actions workspace/output environment is missing.");
  const sha = process.env.CONTAINER_COMMIT_SHA;
  const catalogPath = resolve(workspace, process.env.CONTAINER_CATALOG || "ci/container-images.json");
  const catalog = loadCatalog(catalogPath, workspace);
  const digests = resolveImages(catalog, sha);
  appendFileSync(outputFile, `digests=${JSON.stringify(digests)}\n`, { encoding: "utf8", mode: 0o600 });
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
