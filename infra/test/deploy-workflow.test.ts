import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(process.cwd(), "..");
const deployWorkflow = readFileSync(resolve(root, ".github", "workflows", "deploy.yml"), "utf8");
const initializeWorkflow = readFileSync(resolve(root, ".github", "workflows", "initialize.yml"), "utf8");
const imageWorkflow = readFileSync(resolve(root, ".github", "workflows", "container-images.yml"), "utf8");

function job(workflow: string, name: string, next?: string): string {
  const start = workflow.indexOf(`  ${name}:`);
  const end = next ? workflow.indexOf(`  ${next}:`, start + 1) : workflow.length;
  expect(start).toBeGreaterThanOrEqual(0);
  expect(end).toBeGreaterThan(start);
  return workflow.slice(start, end);
}

describe("container release workflow behavior", () => {
  test("keeps Lambda ZIP packaging separate from ECS image publication", () => {
    const lambdaJob = job(deployWorkflow, "aws-push-lambda", "aws-push-mail-templates");
    expect(lambdaJob).toContain("target/lambda/${BINARY}/bootstrap.zip");
    expect(lambdaJob).not.toMatch(/search-filter-periodic-match|aura-historia-cron/);
    expect(deployWorkflow).toContain("aws-push-lambda.result == 'success'");
    expect(deployWorkflow).not.toContain("aws-push-periodic-matcher");
  });

  test("catalog, resolver, and image-test changes trigger release validation", () => {
    for (const file of ["ci/container-images.json", "ci/container-images.cjs", "ci/container-images*.test.cjs", "ci/container-images/**", ".github/actions/resolve-container-images/**"]) {
      expect(deployWorkflow).toContain(`"${file}"`);
      expect(imageWorkflow).toContain(`"${file}"`);
    }
  });

  test("reconciles catalog repositories once and publishes a real catalog matrix", () => {
    const infra = job(deployWorkflow, "infra-test", "aws-container-artifacts");
    const artifacts = job(deployWorkflow, "aws-container-artifacts", "aws-push-container-images");
    const publisher = job(deployWorkflow, "aws-push-container-images", "aws-push-lambda");
    expect(infra).toContain("container_matrix: ${{ steps.container-catalog.outputs.matrix }}");
    expect(infra).toContain("node ci/container-images.cjs matrix >> \"$GITHUB_OUTPUT\"");
    expect(artifacts).toContain("group: aws-container-artifact-stack");
    expect(artifacts).toContain("aura-historia-periodic-matcher-artifacts");
    expect(publisher).toContain("needs: [infra-test, aws-container-artifacts]");
    expect(publisher).toContain("matrix: ${{ fromJSON(needs.infra-test.outputs.container_matrix) }}");
    expect(publisher).not.toMatch(/outputs:\s*[\s\S]{0,100}image_digest/);
    expect(publisher).not.toMatch(/periodic-matcher|POSTGRES_|GOOGLE_APPLICATION_CREDENTIALS|VERTEX_AI/);
    expect(publisher).toContain("IMAGE_BINARY: ${{ matrix.binary }}");
    expect(deployWorkflow).not.toContain("aws-periodic-matcher-artifact-apply");
  });

  test("bootstraps the account-wide DMS VPC role before deploying the data stack, without changing manual rollback", () => {
    const bootstrap = deployWorkflow.indexOf("bash infra/scripts/ensure-dms-vpc-role.sh");
    const network = deployWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-network"');
    const data = deployWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-data"');
    expect(bootstrap).toBeGreaterThanOrEqual(0);
    expect(network).toBeGreaterThan(bootstrap);
    expect(data).toBeGreaterThan(network);
    expect(deployWorkflow).toMatch(/name: Ensure account-level DMS VPC role\n\s+if: github.event_name == 'push'/);
  });

  test("manual rollback reuses deployed templates without invoking migrations", () => {
    const rollback = deployWorkflow.split('      - name: Preflight stage artifacts and deployed stack state')[1];
    expect(rollback).toBeDefined();
    expect(rollback).toContain('--use-previous-template');
    expect(rollback).toContain('for stack in initialize compute; do');
    expect(rollback).not.toContain('database-migration-lambda');
    expect(rollback).not.toMatch(/aws lambda invoke|invoke_function|npm --prefix infra run cdk -- deploy/);
  });

  test("tests a local image before adding its final immutable SHA tag", () => {
    const publisher = job(deployWorkflow, "aws-push-container-images", "aws-push-lambda");
    const smoke = publisher.indexOf('bash "ci/container-images/${IMAGE_ID}/smoke.sh" "$local_ref"');
    const finalTag = publisher.indexOf('docker tag "$local_ref" "${uri}:${tag}"');
    const push = publisher.indexOf('docker push "${uri}:${tag}"');
    expect(smoke).toBeGreaterThanOrEqual(0);
    expect(finalTag).toBeGreaterThan(smoke);
    expect(push).toBeGreaterThan(finalTag);
    expect(publisher).toContain("push_succeeded=false");
    expect(publisher).toContain("bash \"ci/container-images/${IMAGE_ID}/smoke.sh\" \"${uri}@${digest}\"");
    expect(publisher).toContain("node ci/container-images.cjs resolve-one");
    expect(publisher).toContain("--allow-missing");
    expect(publisher).toContain("docker pull --platform \"$IMAGE_PLATFORM\" \"${uri}@${digest}\"");
    expect(publisher).toContain("Registry digest: ${digest}");
    expect(publisher).not.toMatch(/sed -nE|FROM rust:|FROM debian:/);
  });

  test("builds and smoke-tests catalog images on unprivileged PR CI", () => {
    expect(imageWorkflow).toContain("pull_request:");
    expect(imageWorkflow).toContain("permissions:\n  contents: read");
    expect(imageWorkflow).not.toContain("id-token: write");
    expect(imageWorkflow).toContain("node --test ci/container-images*.test.cjs");
    expect(imageWorkflow).toContain("docker build --platform \"$IMAGE_PLATFORM\"");
    expect(imageWorkflow).toContain('bash ci/container-images/${IMAGE_ID}/smoke.sh "$IMAGE"');
  });

  test("resolves the full digest map before deployment, initialization, or rollback", () => {
    const deploy = job(deployWorkflow, "aws-cdk-deploy");
    expect(deploy).toContain("uses: ./.github/actions/resolve-container-images");
    expect(deploy).toContain("steps.container-images.outputs.digests");
    expect(deploy).toContain("needs.aws-push-container-images.result == 'success'");
    expect(deploy).toContain("task-outputs --stack-file compute-stack.json");
    expect(deploy).toContain("do not rerun migrations or launch another task");

    expect(initializeWorkflow).toContain("uses: ./.github/actions/resolve-container-images");
    expect(initializeWorkflow).toContain("node ci/container-images.cjs initialize-update");
    expect(initializeWorkflow).toContain(".parameters | to_entries[] | \"\\(.key)=\\(.value)\"");
    expect(initializeWorkflow.indexOf("initialize-update")).toBeLessThan(initializeWorkflow.indexOf("Migrate and capture FX"));
    expect(initializeWorkflow).toContain("task-outputs --stack-file compute-stack.json");
    expect(initializeWorkflow).toContain("do not rerun migrations or launch another task");
    expect(initializeWorkflow).not.toContain("aws-periodic-matcher-artifact-apply");
  });

  test("keeps manual artifact rollback on previous templates without image builds or migrations", () => {
    const preflight = deployWorkflow.split("- name: Preflight stage artifacts and deployed stack state")[1]?.split("- name: Update artifact SHA and image digests")[0];
    const rollback = deployWorkflow.split("- name: Update artifact SHA and image digests")[1];
    expect(preflight).toBeDefined();
    expect(rollback).toContain("--use-previous-template");
    expect(rollback).toContain("previous-update");
    expect(rollback).toContain("compute-previous-update.json");
    expect(rollback).not.toMatch(/docker build|docker pull|database-migration-lambda|fxrate-lambda|invoke_function/);
    expect(preflight).not.toMatch(/docker build|docker pull|docker run|sed -nE/);
    expect(preflight).toContain("previous-update");
  });

  test("shares only per-stage deployment locks and does not serialize image publishers globally", () => {
    expect(deployWorkflow).toContain("group: aws-deploy-${{ github.event_name == 'workflow_dispatch' && inputs.stage");
    expect(initializeWorkflow).toContain("group: aws-deploy-${{ inputs.stage }}");
    const publisher = job(deployWorkflow, "aws-push-container-images", "aws-push-lambda");
    expect(publisher).not.toContain("concurrency:");
    expect(initializeWorkflow).not.toContain("concurrency:\n      group:");
  });
});
