import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(process.cwd(), "..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");
const deployWorkflow = read(".github/workflows/deploy.yml");
const imageWorkflow = read(".github/workflows/container-images.yml");
const integrateWorkflow = read(".github/workflows/integrate.yml");
const cdkWorkflow = read(".github/workflows/cdk-test.yml");
const mjmlWorkflow = read(".github/workflows/mjml-templates.yml");
const operationWorkflows = ["migrate", "initialize"].map((operation) => ({
  operation,
  workflow: read(`.github/workflows/${operation}.yml`),
}));
const deployHelper = read("ci/deploy-stacks.sh");
const publishHelper = read("ci/publish-container.sh");
const operationHelper = read("ci/invoke-operation.sh");
const artifactHelper = read("ci/s3-artifact-exists.sh");
const { resolveSource, validateReleaseTag }: {
  resolveSource: (env: Record<string, string>, git: (args: string[]) => string) => { sha: string; ref: string };
  validateReleaseTag: (tag: string) => string;
} = require(resolve(root, "ci/release-source.cjs"));

// Bound assertions to YAML sections without depending on the next job's name or action versions.
function block(source: string, key: string, indent = 0): string {
  const lines = source.split("\n");
  const start = lines.findIndex((line) => line.startsWith(`${" ".repeat(indent)}${key}:`));
  expect(start).toBeGreaterThanOrEqual(0);
  const end = lines.findIndex((line, index) => index > start && new RegExp(`^ {0,${indent}}\\S`).test(line));
  return lines.slice(start, end < 0 ? undefined : end).join("\n");
}

function job(workflow: string, name: string): string {
  return block(block(workflow, "jobs"), name, 2);
}

function jobNames(workflow: string): string[] {
  return [...block(workflow, "jobs").matchAll(/^  ([\w-]+):/gm)].map((match) => match[1]);
}

function needs(source: string): string[] {
  const match = block(source, "needs", 4).match(/needs:\s*\[([^\]]*)\]/);
  expect(match).not.toBeNull();
  return match![1].split(",").map((name) => name.trim()).filter(Boolean);
}

function expectInOrder(source: string, ...parts: string[]): void {
  let previous = -1;
  for (const part of parts) {
    const position = source.indexOf(part);
    expect(position).toBeGreaterThan(previous);
    previous = position;
  }
}

const awsDeployJobs = jobNames(deployWorkflow).filter((name) =>
  job(deployWorkflow, name).includes("aws-actions/configure-aws-credentials@"));
const sha = "a".repeat(40);
const releaseTag = "20260929-1200";

describe("release source contract", () => {
  test("deploys develop to dev and CalVer tags to prod, with manual source and scope selection", () => {
    const triggers = block(deployWorkflow, "on");
    const push = block(triggers, "push", 2);
    expect(push).toMatch(/branches:\s*\[develop\]/);
    expect(push).toContain(`tags: ["${"[0-9]".repeat(8)}-${"[0-9]".repeat(4)}"]`);
    const dispatch = block(triggers, "workflow_dispatch", 2);
    expect(block(dispatch, "stage", 6)).toMatch(/options:\s*\[dev, prod\]/);
    expect(block(dispatch, "ref", 6)).toMatch(/type:\s*string/);
    expect(block(dispatch, "scope", 6)).toMatch(/default:\s*auto/);
    expect(block(dispatch, "scope", 6)).toMatch(/options:\s*\[auto, foundation, all\]/);
    expect(block(deployWorkflow, "env")).toMatch(/STAGE:.*workflow_dispatch.*inputs.stage.*refs\/tags\/.*'prod'.*'dev'/);
    expect(job(deployWorkflow, "aws-cdk-deploy")).toContain("DEPLOY_SCOPE: ${{ inputs.scope || 'auto' }}");
  });

  test.each([
    ["dev", "push", "", "refs/heads/develop", sha],
    ["prod", "push", "", `refs/tags/${releaseTag}`, `refs/tags/${releaseTag}`],
    ["dev", "workflow_dispatch", "", "", "refs/remotes/origin/develop"],
    ["dev", "workflow_dispatch", "develop", "", "refs/remotes/origin/develop"],
    ["dev", "workflow_dispatch", sha, "", sha],
    ["prod", "workflow_dispatch", releaseTag, "", `refs/tags/${releaseTag}`],
  ])("resolves %s %s source '%s' once and requires develop ancestry", (stage, event, requested, ref, selected) => {
    const git = jest.fn(() => sha);
    expect(resolveSource({ STAGE: stage, GITHUB_EVENT_NAME: event, REQUESTED_REF: requested, GITHUB_REF: ref, GITHUB_SHA: sha }, git))
      .toEqual({ sha, ref: selected });
    expect(git).toHaveBeenCalledWith(["rev-parse", "--verify", `${selected}^{commit}`]);
    expect(git).toHaveBeenLastCalledWith(["merge-base", "--is-ancestor", sha, "refs/remotes/origin/develop"]);
  });

  test.each([
    ["dev", "feature-branch"], ["dev", "abc123"], ["dev", "A".repeat(40)],
    ["prod", ""], ["prod", "develop"], ["prod", sha],
  ])("rejects invalid manual %s source '%s' before Git resolution", (stage, requested) => {
    const git = jest.fn(() => sha);
    expect(() => resolveSource({ STAGE: stage, GITHUB_EVENT_NAME: "workflow_dispatch", REQUESTED_REF: requested }, git)).toThrow();
    expect(git).not.toHaveBeenCalled();
  });

  test("rejects an unsupported stage before resolving the source", () => {
    const git = jest.fn(() => sha);
    expect(() => resolveSource({ STAGE: "staging", GITHUB_EVENT_NAME: "workflow_dispatch" }, git)).toThrow(/Stage/);
    expect(git).not.toHaveBeenCalled();
  });

  test.each(["20260230-1200", "20260929-2400", "20260929-1260", "v20260929-1200"])("rejects invalid UTC CalVer tag %s", (tag) => {
    expect(() => validateReleaseTag(tag)).toThrow();
  });

  test("fails closed for moved tags and sources outside develop", () => {
    expect(validateReleaseTag("20240229-2359")).toBe("20240229-2359");
    const env = { STAGE: "prod", GITHUB_EVENT_NAME: "push", GITHUB_REF: `refs/tags/${releaseTag}`, GITHUB_SHA: sha };
    const movedTag = jest.fn().mockReturnValueOnce(sha).mockReturnValueOnce("b".repeat(40));
    expect(() => resolveSource(env, movedTag)).toThrow(/triggering commit/);
    const unrelatedSource = jest.fn((args: string[]) => {
      if (args[0] === "merge-base") throw new Error("Not an ancestor");
      return sha;
    });
    expect(() => resolveSource(env, unrelatedSource)).toThrow("Not an ancestor");
  });
});

describe("change-scoped CI workflows", () => {
  test("compiles templates only for MJML or its workflow changes", () => {
    expect(jobNames(mjmlWorkflow)).toEqual(["mjml-compile"]);
    const triggers = block(mjmlWorkflow, "on");
    for (const event of ["push", "pull_request"]) {
      const paths = block(block(triggers, event, 2), "paths", 4);
      expect(paths).toContain('"mjml/**"');
      expect(paths).toContain('"infra/src/resources/cognito-verification-email.html"');
      expect(paths).toContain('".github/workflows/mjml-templates.yml"');
      expect(paths).not.toContain('"src/**"');
    }
    expectInOrder(
      mjmlWorkflow,
      "git ls-files --error-unmatch -- infra/src/resources/cognito-verification-email.html",
      "npm --prefix mjml run generate:cognito-verification",
      "git diff --exit-code -- infra/src/resources/cognito-verification-email.html",
      "Compile every MJML template",
    );
    expect(jobNames(integrateWorkflow)).not.toContain("mjml-compile");
  });

  test("runs CDK and deployment helper checks for their inputs but not unrelated Rust changes", () => {
    expect(jobNames(cdkWorkflow)).toEqual(["infra-test"]);
    const triggers = block(cdkWorkflow, "on");
    for (const event of ["push", "pull_request"]) {
      const paths = block(block(triggers, event, 2), "paths", 4);
      for (const path of [
        "infra/**",
        "mjml/**",
        "ci/**",
        "docs/swagger.yaml",
        "src/**/Cargo.toml",
        "src/aura-historia-api/src/lib.rs",
        ".github/workflows/deploy.yml",
      ]) {
        expect(paths).toContain(`"${path}"`);
      }
      expect(paths).not.toContain('"src/**"');
    }
    expectInOrder(
      cdkWorkflow,
      "git ls-files --error-unmatch -- infra/src/resources/cognito-verification-email.html",
      "npm --prefix mjml run generate:cognito-verification",
      "git diff --exit-code -- infra/src/resources/cognito-verification-email.html",
      "npm --prefix infra ci",
    );
    expect(jobNames(integrateWorkflow)).not.toContain("infra-test");
    for (const event of ["push", "pull_request"]) {
      const paths = block(block(integrateWorkflow, "on"), event, 2);
      expect(paths).not.toContain('"infra/**"');
      expect(paths).not.toContain('"mjml/**"');
    }
  });

  test("builds container images on workspace changes and deployment workflow changes", () => {
    const triggers = block(imageWorkflow, "on");
    for (const event of ["push", "pull_request"]) {
      const paths = block(block(triggers, event, 2), "paths", 4);
      expect(paths).toContain('"src/**"');
      expect(paths).toContain('".github/workflows/deploy.yml"');
    }
  });
});

describe("deployment workflow boundaries", () => {
  test("resolves and validates the source without credentials before testing the pinned checkout", () => {
    const infra = job(deployWorkflow, "infra-test");
    expect(infra).toContain("fetch-depth: 0");
    expect(infra).toContain("REQUESTED_REF: ${{ inputs.ref }}");
    expect(block(infra, "outputs", 4)).toContain("stage: ${{ env.STAGE }}");
    expect(infra).toContain("commit_sha: ${{ steps.source.outputs.commit_sha }}");
    expect(block(infra, "steps", 4)).not.toMatch(/^\s+STAGE:/m);
    expectInOrder(infra, "node ci/release-source.cjs", "ref: ${{ steps.source.outputs.commit_sha }}", "npm --prefix infra ci", "npm --prefix infra test");
    expect(infra).toContain("node --test ci/*.test.cjs");
    expect(infra).not.toMatch(/configure-aws-credentials|id-token:\s*write/);
  });

  test.each(awsDeployJobs)("configures %s for AWS and gates only the final deployment", (name) => {
    const awsJob = job(deployWorkflow, name);
    expect(needs(awsJob)).toContain("infra-test");
    if (name === "aws-cdk-deploy") {
      expect(block(awsJob, "environment", 4)).toContain("name: aws-${{ needs.infra-test.outputs.stage }}");
    } else {
      expect(awsJob).not.toMatch(/^    environment:/m);
      expect(awsJob).not.toMatch(/\bcdk\b|cloudformation|deploy-stacks\.sh|ensure-dms-vpc-role/);
    }
    expect(awsJob).toMatch(/id-token:\s*write/);
    expect(awsJob).toContain("secrets.CI_DEPLOY_ROLE_ARN");
    expect(awsJob).toContain("vars.AWS_REGION");
  });

  test.each(awsDeployJobs)("pins application job %s to the resolved release", (name) => {
    const awsJob = job(deployWorkflow, name);
    expect(awsJob).toContain("DEPLOY_COMMIT_SHA: ${{ needs.infra-test.outputs.commit_sha }}");
    expectInOrder(awsJob, "ref: ${{ env.DEPLOY_COMMIT_SHA }}", "aws-actions/configure-aws-credentials@");
    expect(awsJob).not.toMatch(/github\.sha|ref:\s*(?:develop|\$\{\{\s*(?:inputs\.ref|github\.ref))/);
  });

  test("uses pre-provisioned repositories without deploying or importing an artifact stack", () => {
    expect(deployWorkflow).not.toMatch(/aws-container-artifacts|aura-historia-container-artifacts|bin\/artifacts\.ts/);
    expect(publishHelper).not.toMatch(/cloudformation|create-repository|put-image-tag-mutability|\bcdk\b/);
    expectInOrder(publishHelper, "aws ecr describe-repositories", '.[0].imageTagMutability == "IMMUTABLE"', "docker login");
    expect(publishHelper).toContain('.[0].registryId == $account');
    expect(publishHelper).toContain('.[0].repositoryUri == $uri');
    expect(publishHelper).toContain('.[0].encryptionConfiguration.encryptionType == "AES256"');
    expect(publishHelper).toContain("infra/README.md#container-repository-setup");
  });

  test("uses one dependency graph for every trigger and requires every publisher to succeed before CDK", () => {
    const prerequisites = ["aws-push-container-images", "aws-push-lambda", "aws-push-mail-templates"];
    expect(awsDeployJobs).toEqual([...prerequisites, "aws-cdk-deploy"]);
    for (const name of prerequisites) {
      expect(needs(job(deployWorkflow, name))).toEqual(["infra-test"]);
    }
    expect(needs(job(deployWorkflow, "aws-cdk-deploy"))).toEqual(["infra-test", ...prerequisites]);
    expect(jobNames(deployWorkflow).filter((name) => /^    environment:/m.test(job(deployWorkflow, name)))).toEqual(["aws-cdk-deploy"]);
    for (const name of jobNames(deployWorkflow)) {
      const body = job(deployWorkflow, name);
      // GitHub's default success condition blocks failed or skipped dependencies.
      expect(body).not.toMatch(/^    if:/m);
      expect(body).not.toMatch(/continue-on-error:\s*true|\balways\(\)/);
    }
  });

  test("shares stage locks with operations without a global artifact-stack lock", () => {
    const concurrency = block(deployWorkflow, "concurrency");
    expect(concurrency).toMatch(/group:\s*aws-deploy-.*inputs.stage.*refs\/tags\/.*'prod'.*'dev'/);
    expect(concurrency).toMatch(/cancel-in-progress:\s*false/);
    expect(deployWorkflow).not.toContain("aws-container-artifact-stack");
    expect(job(deployWorkflow, "aws-push-container-images")).not.toMatch(/^    concurrency:/m);
  });


  test("keeps release validation responsive to workflows, helper scripts and catalog changes", () => {
    const push = block(block(deployWorkflow, "on"), "push", 2);
    for (const path of [".github/workflows/**", ".github/actions/**", "ci/**"]) expect(push).toContain(`"${path}"`);
    for (const path of ["ci/container-images.json", "ci/container-images.cjs", "ci/container-images*.test.cjs", "ci/container-images/**", ".github/actions/resolve-container-images/**"]) {
      expect(imageWorkflow).toContain(`"${path}"`);
    }
  });
});

describe("immutable artifact publication", () => {
  test("reads the Lambda matrix from the release-pinned catalog instead of the running workflow", () => {
    const infra = job(deployWorkflow, "infra-test");
    expect(block(infra, "outputs", 4)).toContain("lambda_matrix: ${{ steps.lambda-catalog.outputs.matrix }}");
    expectInOrder(infra, "ref: ${{ steps.source.outputs.commit_sha }}", "id: lambda-catalog", "./ci/lambda-binaries.json");
    const catalogStep = infra.split(/^      - /m).find((step) => step.includes("id: lambda-catalog"));
    expect(catalogStep).toContain('JSON.stringify({binary: require("./ci/lambda-binaries.json")})');
    expect(catalogStep).toContain('"matrix="');
    expect(catalogStep).toContain('>> "$GITHUB_OUTPUT"');
    const lambda = job(deployWorkflow, "aws-push-lambda");
    expect(block(lambda, "strategy", 4)).toContain("matrix: ${{ fromJSON(needs.infra-test.outputs.lambda_matrix) }}");
    expect(block(lambda, "strategy", 4)).not.toMatch(/^\s+binary:/m);
    expect(lambda).toContain("BINARY: ${{ matrix.binary }}");
  });

  test("catalogs unique workspace binary crates with matching package names and source entry points", () => {
    const binaries: string[] = JSON.parse(read("ci/lambda-binaries.json"));
    expect(Array.isArray(binaries)).toBe(true);
    expect(binaries).toContain("product-listing-ingestion-lambda");
    expect(binaries).not.toContain("aura-historia-worker");
    expect(new Set(binaries).size).toBe(binaries.length);
    const members = read("Cargo.toml").match(/\[workspace\]\s*members\s*=\s*\[([^\]]*)\]/)?.[1];
    expect(members).toBeDefined();
    for (const binary of binaries) {
      expect(binary).toMatch(/^[a-z][a-z0-9-]*$/);
      const crate = `src/${binary}`;
      expect(members).toContain(`"${crate}"`);
      const manifest = read(`${crate}/Cargo.toml`);
      expect(manifest.match(/^name\s*=\s*"([^"]+)"/m)?.[1]).toBe(binary);
      expect(manifest).not.toMatch(/^autobins\s*=\s*false/m);
      expect(existsSync(resolve(root, crate, "src/main.rs"))).toBe(true);
    }
  });

  test("matches the CDK runtime definitions and initialization ZIP artifacts without another full synth", () => {
    const binaries: string[] = JSON.parse(read("ci/lambda-binaries.json"));
    const definitions = read("infra/src/constructs/lambdas.ts");
    // The definitions are private; include both the runtime catalog and direct initialization S3 keys.
    const runtimeBinaries = [...definitions.matchAll(/\bbinaryName:\s*"([^"]+)"/g)].map((match) => match[1]);
    const initializationBinaries = [...definitions.matchAll(/`([a-z][a-z0-9-]+)-\$\{[^}]+\}-\$\{[^}]+\}\.zip`/g)].map((match) => match[1]);
    expect(runtimeBinaries.length).toBeGreaterThan(0);
    expect(initializationBinaries.length).toBeGreaterThan(0);
    expect([...binaries].sort()).toEqual([...runtimeBinaries, ...initializationBinaries].sort());
  });

  test("keeps Lambda ZIPs separate from catalog images and builds/uploads only missing ZIPs", () => {
    const lambda = job(deployWorkflow, "aws-push-lambda");
    expect(lambda).toContain("target/lambda/${BINARY}/bootstrap.zip");
    expect(lambda).not.toMatch(/search-filter-periodic-match|aura-historia-cron|aura-historia-worker|sequin|docker build/i);
    expect(lambda).toContain('bash ci/s3-artifact-exists.sh "$BUCKET" "${BINARY}-${STAGE}-${DEPLOY_COMMIT_SHA}.zip"');
    const steps = lambda.split(/^      - /m);
    const build = steps.find((step) => step.includes("cargo lambda build"));
    const upload = steps.find((step) => step.includes("aws s3api put-object"));
    expect(build).toContain("working-directory: src/${{ matrix.binary }}");
    expect(build).toContain('--bin "$BINARY"');
    expect(upload).not.toContain("working-directory:");
    expect(upload).toContain('file="target/lambda/${BINARY}/bootstrap.zip"');
    for (const command of ["cargo lambda build", "aws s3api put-object"]) {
      const step = steps.find((candidate) => candidate.includes(command));
      expect(step).toBeDefined();
      expect(step).toContain("if: steps.artifact.outputs.exists == 'false'");
    }
    expect(lambda).toContain("--if-none-match '*'");
    expect(lambda).not.toMatch(/aws s3 (?:cp|sync|rm)|s3api delete-object/);
  });

  test("compiles and conditionally uploads only missing SHA-scoped mail templates", () => {
    const mail = job(deployWorkflow, "aws-push-mail-templates");
    expectInOrder(mail, 'key="${STAGE}/${DEPLOY_COMMIT_SHA}/${template%.mjml}.html"', "bash ci/s3-artifact-exists.sh", 'if [ "$exists" = false ]; then', "aws s3api put-object");
    const missing = mail.match(/if \[ "\$exists" = false \]; then([\s\S]*?)^\s*fi$/m)?.[1];
    expect(missing).toBeDefined();
    expect(missing).toContain('mjml/node_modules/.bin/mjml "$template"');
    expect(missing).toContain('aws s3api put-object --bucket "$BUCKET" --key "$key"');
    expect(missing).toContain("--if-none-match '*'");
    expect(mail).not.toMatch(/aws s3 (?:cp|sync|rm)|s3api delete-object/);
    expect(artifactHelper).toContain("aws s3api head-object");
    expect(artifactHelper).toContain("404|NoSuchKey|NotFound");
    expect(artifactHelper).toMatch(/then\s+echo true/);
    expect(artifactHelper).toMatch(/else\s+cat[^\n]+\n\s+exit 1/);
  });

  test("delegates catalog-driven publication to the generic helper without deploying repositories", () => {
    const infra = job(deployWorkflow, "infra-test");
    const publisher = job(deployWorkflow, "aws-push-container-images");
    expect(infra).toContain("container_matrix: ${{ steps.container-catalog.outputs.matrix }}");
    expect(infra).toContain("node ci/container-images.cjs matrix");
    expect(publisher).toContain("matrix: ${{ fromJSON(needs.infra-test.outputs.container_matrix) }}");
    for (const field of ["id", "binary", "repository", "dockerfile", "platform"]) {
      expect(publisher).toContain(`IMAGE_${field.toUpperCase()}: \${{ matrix.${field} }}`);
    }
    expect(publisher).toContain("bash ci/publish-container.sh");
    expect(publisher).not.toMatch(/docker (?:build|push)|periodic-matcher|POSTGRES_|GOOGLE_APPLICATION_CREDENTIALS|VERTEX_AI/);
    expect(publishHelper).not.toMatch(/periodic-matcher|FROM rust:|FROM debian:/);
  });

  test("reuses trusted immutable images and smoke-tests new images before tagging and digest verification", () => {
    expect(publishHelper).toContain('tag="git-${DEPLOY_COMMIT_SHA}"');
    expect(publishHelper).toContain("node ci/container-images.cjs resolve-one");
    expectInOrder(publishHelper, 'initial="$(resolve_tag --allow-missing)"', 'verify_image "${uri}@${digest}" reused-image.json', "docker build");
    expectInOrder(publishHelper, 'bash "ci/container-images/${IMAGE_ID}/smoke.sh" "$local_ref"', 'docker tag "$local_ref" "${uri}:${tag}"', 'docker push "${uri}:${tag}"', 'published="$(resolve_tag --allow-missing)"', 'verify_image "${uri}@${digest}" published-image.json');
    expect(publishHelper).toContain('docker pull --platform "$IMAGE_PLATFORM" "${uri}@${digest}"');
    expect(publishHelper).toContain("push_succeeded=false");
    expect(publishHelper).toContain('bash "ci/container-images/${IMAGE_ID}/smoke.sh" "${uri}@${digest}"');
    expect(publishHelper).toContain("^sha256:[0-9a-f]{64}$");
    expect(publishHelper).not.toMatch(/batch-delete-image|put-image-tag-mutability/);
  });

  test("builds and smoke-tests the catalog on unprivileged PR CI", () => {
    expect(block(imageWorkflow, "on")).toContain("pull_request:");
    expect(block(imageWorkflow, "permissions")).toMatch(/contents:\s*read/);
    expect(imageWorkflow).not.toMatch(/id-token:\s*write|configure-aws-credentials|docker push/);
    expect(imageWorkflow).toContain("node --test ci/container-images*.test.cjs");
    expect(imageWorkflow).toContain("fromJSON(needs.catalog.outputs.container_matrix)");
    expectInOrder(imageWorkflow, 'docker build --platform "$IMAGE_PLATFORM"', 'bash ci/container-images/${IMAGE_ID}/smoke.sh "$IMAGE"');
  });
});

describe("CDK deployment responsibility", () => {
  test("resolves the pinned image map then delegates deployment, never manual operations or imports", () => {
    const deploy = job(deployWorkflow, "aws-cdk-deploy");
    const checkout = deploy.split(/^      - /m).find((step) => step.includes("uses: actions/checkout@"));
    expect(checkout).toContain("ref: ${{ env.DEPLOY_COMMIT_SHA }}");
    expect(checkout).toMatch(/fetch-depth:\s*0/);
    expectInOrder(deploy, "uses: ./.github/actions/resolve-container-images", "bash ci/deploy-stacks.sh");
    expect(deploy).toContain("commit-sha: ${{ env.DEPLOY_COMMIT_SHA }}");
    expect(deploy).toContain("CONTAINER_IMAGE_DIGESTS: ${{ steps.container-images.outputs.digests }}");
    expect(deployHelper).toContain('npm --prefix infra run cdk -- deploy "$@"');
    for (const source of [deployWorkflow, deployHelper]) {
      expect(source).not.toMatch(/aws lambda invoke|invoke-operation\.sh|--use-previous-template|--resources-to-import|--change-set-type\s+IMPORT|\bcdk\s+import\b/);
    }
  });

  test("compares deployed and selected migration sources only in auto, failing closed before mutations", () => {
    const gate = deployHelper.match(/^if \[ "\$DEPLOY_SCOPE" = auto \] && \[ "\$application_exists" = true \]; then[\s\S]*?^fi$/m)?.[0];
    expect(gate).toBeDefined();
    expect(gate).toContain('select(.ParameterKey == "CommitSHA")');
    expect(gate).toContain("compute-stack.json");
    expect(gate).toContain('[[ "$current_sha" =~ ^[0-9a-f]{40}$ ]]');
    expect(gate).toContain('git --no-pager diff --quiet "$current_sha" "$DEPLOY_COMMIT_SHA"');
    expect(gate).toContain("-- migrations infra/sql src/database-migration-lambda");
    expectInOrder(gate!, "compute-stack.json", '[[ "$current_sha"', "git --no-pager diff", "status=$?", 'if [ "$status" != 1 ]; then', "exit 1", "migration_changed=true");
    // Explicit all scope bypasses this source-only check, not an assertion of database readiness.
    expectInOrder(deployHelper, "migration_changed=false", gate!, "bash infra/scripts/ensure-dms-vpc-role.sh", 'deploy "${STACK_NAME_PREFIX}-network"');
  });

  test("stops at foundation for new stages or changed migration sources until explicit all acknowledgement", () => {
    expect(deployHelper).toContain('case "$DEPLOY_SCOPE" in auto|foundation|all)');
    expect(deployHelper).toContain("application_exists=false");
    expect(deployHelper).toContain('if [ "$stack" = compute ]; then application_exists=true; fi');
    const guard = deployHelper.match(/^if .*"\$DEPLOY_SCOPE".*foundation[\s\S]*?^fi$/m)?.[0];
    expect(guard).toBeDefined();
    expect(guard).toContain('[ "$DEPLOY_SCOPE" = auto ] && [ "$application_exists" = false ]');
    expect(guard).toContain('|| [ "$migration_changed" = true ] ||');
    expect(guard).toContain("exit 0");
    expect(guard).toContain("scope=all");
    expectInOrder(deployHelper, "aws cloudformation describe-stacks", "bash infra/scripts/ensure-dms-vpc-role.sh", 'deploy "${STACK_NAME_PREFIX}-network"', 'deploy "${STACK_NAME_PREFIX}-data"', 'deploy "${STACK_NAME_PREFIX}-initialize"', guard!, 'deploy "${STACK_NAME_PREFIX}-compute"', 'deploy "${STACK_NAME_PREFIX}-api"');
    expect(deployHelper.match(/bash infra\/scripts\/ensure-dms-vpc-role\.sh/g)).toHaveLength(1);
    expect(deployWorkflow).not.toContain("ensure-dms-vpc-role.sh");
    for (const { workflow } of operationWorkflows) expect(workflow).not.toContain("ensure-dms-vpc-role.sh");
    expect(deployHelper).toMatch(/if \[ "\$STAGE" = prod \]; then deploy "\$\{STACK_NAME_PREFIX\}-observability"/);
  });

  test("preserves existing activation parameters while updating source and image digests", () => {
    expect(deployHelper).toContain("--previous-parameters");
    expect(deployHelper).toContain("--exclusively");
    expect(deployHelper).toContain("node ci/container-images.cjs deploy-parameters");
    expect(deployHelper).toContain('--commit-sha "$DEPLOY_COMMIT_SHA"');
    expect(deployHelper).toContain("--digests container-image-digests.json");
    expect(deployHelper).toContain('deploy "${STACK_NAME_PREFIX}-compute" "${compute_parameters[@]}"');
    expect(deployHelper).not.toMatch(/(?:CdcRouterEnabled|PeriodicMatcherEnabled)=/);
  });
});

describe("manual operations responsibility", () => {
  test.each(operationWorkflows)("$operation is manual-only, environment-gated, serialized and invocation-only", ({ operation, workflow }) => {
    const events = [...block(workflow, "on").matchAll(/^  ([\w-]+):/gm)].map((match) => match[1]);
    expect(events).toEqual(["workflow_dispatch"]);
    expect(block(workflow, "commit_sha", 6)).toMatch(/required:\s*true/);
    expect(block(workflow, "commit_sha", 6)).toMatch(/type:\s*string/);
    expect(block(workflow, "env")).toContain("STAGE: ${{ inputs.stage }}");
    expect(block(workflow, "env")).toContain("DEPLOY_COMMIT_SHA: ${{ inputs.commit_sha }}");
    const concurrency = block(workflow, "concurrency");
    expect(concurrency).toContain("group: aws-deploy-${{ inputs.stage }}");
    expect(concurrency).toMatch(/cancel-in-progress:\s*false/);
    for (const name of jobNames(workflow)) {
      const operationJob = job(workflow, name);
      expect(block(operationJob, "environment", 4)).toContain("name: aws-${{ inputs.stage }}");
      expect(operationJob).toMatch(/id-token:\s*write/);
      expect(operationJob).toContain("secrets.CI_DEPLOY_ROLE_ARN");
      expectInOrder(operationJob, "ref: ${{ github.sha }}", "aws-actions/configure-aws-credentials@", `bash ci/invoke-operation.sh ${operation}`);
    }
    expect(workflow).not.toMatch(/ref:\s*\$\{\{\s*inputs\.commit_sha|\bnpm\b|\bcdk\b|resolve-container-images|release-source|publish-container|docker|deploy-stacks/);
    expect(workflow).not.toContain(`invoke-operation.sh ${operation === "migrate" ? "initialize" : "migrate"}`);
  });

  test("checks the deployed initialization SHA before invoking only the selected operation", () => {
    expect(operationHelper).toContain("^[0-9a-f]{40}$");
    expect(operationHelper).toContain('stack_name="application-${STAGE}-initialize"');
    expectInOrder(operationHelper, "aws cloudformation describe-stacks", 'json_matches stack "$work_dir/stack.json"', 'json_matches sha "$work_dir/stack.json" "$DEPLOY_COMMIT_SHA"', "aws lambda wait function-updated", "aws lambda invoke");
    expect(operationHelper).toContain("parameter.ParameterKey === 'CommitSHA'");
    expect(operationHelper).toContain("commits.length === 1 && commits[0].ParameterValue === expected");
    expect(operationHelper).toContain('function_name="database-migration-lambda-${STAGE}"');
    expect(operationHelper).toContain('function_name="fxrate-lambda-${STAGE}"');
    expect(operationHelper).toContain("deployment:fxrate:initial:${process.env.STAGE}:v1");
    expect(operationHelper).toContain("--invocation-type RequestResponse");
    expect(operationHelper).not.toMatch(/\bnpm\b|\bcdk\b|resolve-container-images|deploy-stacks|aws cloudformation (?:deploy|update-stack|create-stack|create-change-set|execute-change-set)/);
  });
});
