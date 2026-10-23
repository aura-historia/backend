import { readFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(__dirname, "../..");
const read = (path: string) => readFileSync(resolve(root, path), "utf8");

test("the sole normal GitHub entry is manual, exact-source tested and protected before building or target mutation", () => {
  const workflow = read(".github/workflows/deploy.yml");
  expect(workflow).toMatch(/workflow_dispatch:/);
  expect(workflow).not.toMatch(/^  (push|pull_request|schedule):/m);
  expect(workflow).toContain("./.github/workflows/release-tests.yml");
  expect(workflow).toContain("environment: aws-release-build");
  expect(workflow).toContain("aws-${{ inputs.target }}");
  expect(workflow).not.toMatch(/cdk deploy|publish-container|invoke-operation|release\.cjs|dms (stop|start)-replication/);
  const tests = read(".github/workflows/release-tests.yml");
  expect(tests).toContain("source_sha");
  expect(tests).not.toMatch(/paths-filter|affected-crate|continue-on-error: true/);
});

test("retired routine mutation entrypoints cannot compete with the native pipeline", () => {
  for (const file of ["ci/release.cjs", "ci/deploy-stacks.sh", "ci/invoke-operation.sh", "infra/bin/releases.ts",
    ".github/workflows/migrate.yml", ".github/workflows/initialize.yml", ".github/workflows/release-adopt.yml"])
    expect(existsSync(resolve(root, file))).toBe(false);
  const maintenance = read(".github/workflows/maintenance.yml");
  expect(maintenance).toContain("workflow_dispatch:");
  expect(maintenance).toContain("-maintenance");
});
