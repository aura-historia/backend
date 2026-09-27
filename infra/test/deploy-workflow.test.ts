import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const deployWorkflow = readFileSync(
  resolve(process.cwd(), "..", ".github", "workflows", "deploy.yml"),
  "utf8",
);
const initializeWorkflow = readFileSync(
  resolve(process.cwd(), "..", ".github", "workflows", "initialize.yml"),
  "utf8",
);

describe("deployment workflow boundary", () => {
  test("packages exactly the CDK Lambda artifacts, not the legacy native worker", () => {
    const matrix = deployWorkflow.split("  aws-push-lambda:")[1]?.split("    steps:")[0];
    expect(matrix).toBeDefined();
    const included = [...matrix!.matchAll(/^\s+- crate: src\/([a-z0-9-]+)\n\s+binary: ([a-z0-9-]+)$/gm)];
    const artifacts = included.length
      ? included.map(([, crate, binary]) => {
          expect(crate).toBe(binary);
          return binary;
        })
      : [...matrix!.matchAll(/^\s+- ([a-z0-9-]+)$/gm)].map(([, binary]) => binary);
    const binaries = [
      "aura-historia-api",
      "shopify-lambda",
      "cognito-post-confirmation",
      "cloudwatch-log-retention-lambda",
      "database-migration-lambda",
      "fxrate-lambda",
      "backend-cleanup-lambda",
      "stripe-lambda",
      "product-listing-opensearch-lambda",
      "product-listing-normalization-lambda",
      "product-content-assessment-lambda",
      "product-embedding-lambda",
      "product-translation-lambda",
      "search-filter-projection-lambda",
      "search-filter-percolator-lambda",
      "search-filter-match-notification-lambda",
      "watchlist-notification-lambda",
      "notification-delivery-lambda",
      "cdc-router-lambda",
    ];
    expect(artifacts.sort()).toEqual(binaries.sort());
    expect(deployWorkflow).toMatch(/working-directory: (?:src\/\$\{\{ matrix\.binary \}\}|\$\{\{ matrix\.crate \}\})/);
    expect(deployWorkflow).toContain('target/lambda/${BINARY}/bootstrap.zip');
    expect(deployWorkflow).not.toMatch(/aura-historia-worker|sequin|AURA_HISTORIA_WORKER_/i);
    expect(initializeWorkflow).not.toMatch(/aura-historia-worker|sequin|AURA_HISTORIA_WORKER_/i);
  });

  test("publishes stage/SHA artifacts and deploys only after checks and uploads", () => {
    expect(deployWorkflow).toContain("github.event_name == 'workflow_dispatch'");
    expect(deployWorkflow).toContain("cancel-in-progress: false");
    expect(deployWorkflow).toContain("stage:");
    expect(deployWorkflow).toContain("commit_sha:");
    expect(deployWorkflow).toContain("database-migration-lambda");
    expect(deployWorkflow).toContain('--bin "${{ matrix.binary }}"');
    expect(deployWorkflow).toContain("ref: ${{ env.DEPLOY_COMMIT_SHA }}");
    expect(deployWorkflow).toContain('key="${BINARY}-${STAGE}-${DEPLOY_COMMIT_SHA}.zip"');
    expect(deployWorkflow).toContain("git ls-files -z -- 'mjml/**/*.mjml'");
    expect(deployWorkflow).toContain('name="${template%.mjml}.html"');
    expect(deployWorkflow).toContain('key="${STAGE}/${DEPLOY_COMMIT_SHA}/${name}"');
    expect(deployWorkflow).toContain('key="${STAGE}/${DEPLOY_COMMIT_SHA}/${template%.mjml}.html"');
    expect(deployWorkflow).toMatch(/needs:\s*\[\s*infra-test,\s*aws-push-lambda,\s*aws-push-mail-templates,\s*aws-push-periodic-matcher,?\s*\]/);
    expect(deployWorkflow).toContain("needs.infra-test.result == 'success'");
    expect(deployWorkflow).toContain("needs.aws-push-lambda.result == 'success'");
    expect(deployWorkflow).toContain("needs.aws-push-mail-templates.result == 'success'");
    expect(deployWorkflow).toContain('secrets.CI_DEPLOY_ROLE_ARN');
    expect(deployWorkflow).toContain('migration-result.json');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-compute:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-compute:PeriodicMatcherImageDigest=${PERIODIC_MATCHER_IMAGE_DIGEST}"');
    expect(deployWorkflow).toContain('deploy "${STACK_NAME_PREFIX}-initialize"');

    expect(deployWorkflow).toContain('aws lambda wait function-updated --function-name "database-migration-lambda-${STAGE}"');
    expect(deployWorkflow).toContain("Private migration failed; application admission is blocked.");
    expect(deployWorkflow).not.toContain("deployment_phase:");
    expect(deployWorkflow).not.toContain("dms_initial_cdc_start_position:");
  });

  test("manual rollback reuses deployed templates without invoking migrations", () => {
    const rollback = deployWorkflow.split('      - name: Preflight stage artifacts and deployed stack state')[1];
    expect(rollback).toBeDefined();
    expect(rollback).toContain('--use-previous-template');
    expect(rollback).toContain('for stack in initialize compute; do');
    expect(rollback).not.toContain('database-migration-lambda');
    expect(rollback).not.toMatch(/aws lambda invoke|invoke_function|npm --prefix infra run cdk -- deploy/);
  });

  test("gates push apply on a verified image publisher, but skips publishing on manual dispatch", () => {
    const publisher = deployWorkflow.split("  aws-push-periodic-matcher:")[1]?.split(/^  [a-z-]+:/m)[0];
    const apply = deployWorkflow.split("  aws-cdk-deploy:")[1];
    expect(publisher).toBeDefined();
    expect(publisher).toMatch(/if: github\.event_name == 'push'/);
    expect(publisher).toMatch(/needs:\s*\[infra-test\]/);
    expect(publisher).toContain("steps.image.outputs.image_digest");
    expect(apply).toContain("needs.aws-push-periodic-matcher.result == 'success'");
    expect(apply).toContain("needs.aws-push-periodic-matcher.result == 'skipped'");
    expect(apply).toContain("needs.aws-push-periodic-matcher.outputs.image_digest");
    expect(apply).toContain("needs.infra-test.result == 'success'");
    expect(deployWorkflow).toContain('group: aws-periodic-matcher-artifact-apply');
    expect(initializeWorkflow).toContain('group: aws-periodic-matcher-artifact-apply');
    expect(deployWorkflow).toContain('cancel-in-progress: false');
    expect(initializeWorkflow).toContain('cancel-in-progress: false');
    expect(deployWorkflow).toContain('name: ${{ github.ref == \'refs/heads/prod\' && \'aws-prod\' || \'aws-dev\' }}');
    expect(initializeWorkflow).toContain('id-token: write');
  });

  test("bootstraps an independent retained repository and builds only on actual ECR image absence", () => {
    const publisher = deployWorkflow.split("  aws-push-periodic-matcher:")[1]!.split(/^  [a-z-]+:/m)[0];
    expect(publisher.indexOf("npm --prefix infra ci")).toBeLessThan(publisher.indexOf("aura-historia-periodic-matcher-artifacts"));
    expect(publisher.indexOf("aura-historia-periodic-matcher-artifacts")).toBeLessThan(publisher.indexOf("docker build"));
    expect(publisher).toContain("--app 'npx ts-node --prefer-ts-exts bin/artifacts.ts'");
    expect(publisher).toContain('test "$(git rev-parse HEAD)" = "$DEPLOY_COMMIT_SHA"');
    expect(publisher).toContain('tag="git-${DEPLOY_COMMIT_SHA}"');
    expect(publisher).toContain("ImageNotFoundException");
    expect(publisher).toMatch(/else\s+cat image-error\.txt >&2\s+exit 1/);
    expect(publisher).toContain("docker build --platform linux/amd64");
    expect(publisher).toContain('--build-arg "SOURCE_REVISION=${DEPLOY_COMMIT_SHA}"');
    expect(publisher).toContain('docker pull --platform linux/amd64 "${uri}@${digest}"');
    expect(publisher).toContain('echo "image_digest=$digest" >> "$GITHUB_OUTPUT"');
    expect(deployWorkflow).toContain('".dockerignore"');
    expect(deployWorkflow).toContain('"src/aura-historia-cron/Dockerfile"');
    expect(deployWorkflow).toContain('".github/workflows/initialize.yml"');
  });

  test("re-reads and fully verifies a competing immutable tag instead of trusting push failures", () => {
    const publisher = deployWorkflow.split("  aws-push-periodic-matcher:")[1]!.split(/^  [a-z-]+:/m)[0];
    const push = publisher.indexOf('if ! docker push "${uri}:${tag}"');
    const reread = publisher.indexOf('aws ecr describe-images', push);
    const digest = publisher.indexOf('[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]');
    const pulled = publisher.indexOf('docker pull --platform linux/amd64 "${uri}@${digest}"');
    const checked = publisher.indexOf("'length == 1 and .[0].Os", pulled);
    const smoke = publisher.indexOf('smoke-startup.log', checked);
    const output = publisher.indexOf('echo "image_digest=$digest" >> "$GITHUB_OUTPUT"');
    expect(push).toBeGreaterThan(0);
    expect(publisher.indexOf('built.json >/dev/null')).toBeLessThan(push);
    expect(publisher.indexOf('built.json >/dev/null')).toBeGreaterThan(publisher.indexOf('docker image inspect "${uri}:${tag}"'));
    expect(reread).toBeGreaterThan(push);
    expect(publisher).toContain('Push failed but the SHA tag exists; independently verifying its registry image.');
    expect(publisher).toMatch(/aws ecr describe-images[\s\S]*--output json > image\.json 2>image-error\.txt \|\| \{ cat image-error\.txt >&2; exit 1; \}/);
    expect(publisher).toContain('--build-arg "BUILD_WORKFLOW_IDENTITY=${build_workflow_identity}"');
    expect(reread).toBeGreaterThan(push);
    expect(digest).toBeGreaterThan(reread);
    expect(pulled).toBeGreaterThan(digest);
    expect(checked).toBeGreaterThan(pulled);
    expect(smoke).toBeGreaterThan(checked);
    expect(output).toBeGreaterThan(smoke);
    expect(publisher).toContain("--label \"com.aura-historia.builder-base.digest=${builder_base}\"");
    expect(publisher).toContain("--label \"org.opencontainers.image.base.digest=${runtime_base}\"");
  });

  test("checks pinned base and OCI provenance plus network-isolated non-root smoke on all paths", () => {
    const publisher = deployWorkflow.split("  aws-push-periodic-matcher:")[1]!.split(/^  [a-z-]+:/m)[0];
    const manual = deployWorkflow.split('      - name: Preflight stage artifacts and deployed stack state')[1]!;
    const init = initializeWorkflow.split('      - name: Preflight existing periodic matcher image')[1]!;
    for (const path of [publisher, manual, init]) {
      expect(path).toContain('git show "${DEPLOY_COMMIT_SHA}:src/aura-historia-cron/Dockerfile"');
      expect(path).toContain("sed -nE 's/^FROM rust:");
      expect(path).toContain("sed -nE 's/^FROM debian:");
      expect(path).toContain('"$builder_base" =~ ^sha256:[0-9a-f]{64}$');
      expect(path).toContain('.Config.Labels["org.opencontainers.image.source"] == "https://github.com/aura-historia/backend"');
      expect(path).toContain('.Config.Labels["com.aura-historia.build-workflow"]');
      expect(path).toContain('.Config.Labels["com.aura-historia.builder-base.digest"] == $builder');
      expect(path).toContain('.Config.Labels["org.opencontainers.image.base.digest"] == $runtime');
      expect(path).toContain('.Config.User == "10001:10001"');
      expect(path).toContain('.Config.ExposedPorts // {} | length == 0');
      expect(path).toContain('.Config.Volumes // {} | keys == ["/tmp"]');
      expect(path).toContain('.Config.Entrypoint == ["/usr/local/bin/aura-historia-cron"]');
      expect(path).toContain('(.[0].Config.Cmd // []) == []');
      expect(path).toContain('timeout 20s docker run --rm --name "$container" --network none --read-only');
      expect(path).toContain('"$status" -eq 1');
      expect(path).toContain("grep -q 'accepts no arguments' smoke-args.log");
      expect(path).toContain("grep -q 'startup_failed' smoke-startup.log");
      expect(path).toContain('--mount "type=volume,src=${volume},dst=/tmp"');
      expect(path).toContain('AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON={"type":"authorized_user"}');
      expect(path).toContain("test \"$dir_mode\" = '700:10001:10001'");
      expect(path).toContain("test \"$file_mode\" = '600:10001:10001'");
      expect(path).toContain("--entrypoint /usr/bin/cat");
      expect(path).toContain('docker volume rm -f "$volume"');
      expect(path).toContain('trap ');
      expect(path).toContain("test \"$rds_ca_sha\" = \"$expected_ca\"");
    }
    expect(manual).not.toContain('docker build');
    expect(init).not.toContain('docker build');
    expect(manual.indexOf('smoke-startup.log')).toBeLessThan(manual.indexOf('aws cloudformation update-stack'));
    expect(init.indexOf('smoke-startup.log')).toBeLessThan(init.indexOf('invoke_function "database-migration-lambda-${STAGE}"'));
  });

  test("records registry digest and non-secret release evidence for built and reused images", () => {
    const publisher = deployWorkflow.split("  aws-push-periodic-matcher:")[1]!.split(/^  [a-z-]+:/m)[0];
    const evidence = publisher.indexOf('cat >> "$GITHUB_STEP_SUMMARY"');
    expect(evidence).toBeGreaterThan(publisher.indexOf('docker pull --platform linux/amd64'));
    expect(publisher).toContain('Registry digest: ${digest}');
    expect(publisher).toContain('Registry: ${uri}');
    expect(publisher).toContain('Source SHA: ${DEPLOY_COMMIT_SHA}');
    expect(publisher).toContain('Platform: linux/amd64');
    expect(publisher).toContain('OCI source: https://github.com/aura-historia/backend');
    expect(publisher).toContain('OCI revision: ${DEPLOY_COMMIT_SHA}');
    expect(publisher).toContain('Builder base digest: ${builder_base}');
    expect(publisher).toContain('Runtime base digest: ${runtime_base}');
    expect(publisher).toContain('Build workflow: ${image_build_workflow}');
    expect(publisher).toContain('Verification workflow: ${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID} (attempt ${GITHUB_RUN_ATTEMPT})');
    expect(publisher).toContain('Executable SHA256: ${binary_sha}');
    expect(publisher).toContain('Public RDS CA SHA256: ${rds_ca_sha}');
    expect(publisher).not.toMatch(/Registry digest:.*(?:\.Id|image ID)/);
  });

  test("rejects bad source, platform, runtime and digest on reuse without manual rebuild", () => {
    for (const workflow of [deployWorkflow, initializeWorkflow]) {
      expect(workflow).toContain('aws ecr describe-images --repository-name "$repository" --image-ids imageTag="$tag"');
      expect(workflow).toContain('^sha256:[0-9a-f]{64}$');
      expect(workflow).toContain('.[0].Os == "linux"');
      expect(workflow).toContain('.[0].Architecture == "amd64"');
      expect(workflow).toContain('.[0].Config.Labels["org.opencontainers.image.revision"] == $sha');
      expect(workflow).toContain('.[0].Config.Entrypoint == ["/usr/local/bin/aura-historia-cron"]');
      expect(workflow).toContain('(.[0].Config.Cmd // []) == []');
      expect(workflow).toContain('docker pull --platform linux/amd64');
    }
    const manual = deployWorkflow.split('      - name: Preflight stage artifacts and deployed stack state')[1];
    const init = initializeWorkflow.split('      - name: Preflight existing periodic matcher image')[1];
    expect(manual).not.toContain('docker build');
    expect(init).not.toContain('docker build');
    expect(manual!.indexOf('docker image inspect')).toBeLessThan(manual!.indexOf('aws cloudformation update-stack'));
    expect(init!.indexOf('docker image inspect')).toBeLessThan(init!.indexOf('invoke_function "database-migration-lambda-${STAGE}"'));
  });

  test("manual rollback updates SHA and digest together while retaining all other parameters", () => {
    const manual = deployWorkflow.split('      - name: Preflight stage artifacts and deployed stack state')[1]!;
    expect(manual).toContain('Unsupported historical compute template');
    expect(manual).toContain('PeriodicMatcherEnabled PeriodicMatcherImageDigest');
    expect(manual).toContain('"$current_digest" = "$PERIODIC_MATCHER_IMAGE_DIGEST"');
    expect(manual).toContain('if .ParameterKey == "CommitSHA"');
    expect(manual).toContain('elif .ParameterKey == "PeriodicMatcherImageDigest"');
    expect(manual).toContain('{ParameterKey, UsePreviousValue: true}');
    expect(manual).toContain('--use-previous-template');
    expect(initializeWorkflow).toContain('Unsupported historical compute template');
    expect(initializeWorkflow).toContain('--previous-parameters');
    expect(initializeWorkflow).not.toContain('PeriodicMatcherEnabled=true');
  });

  test("does not restore inventory, sealing or promotion jobs", () => {
    expect(deployWorkflow).not.toMatch(/^  [\w-]*(?:inventory|seal|promot)[\w-]*:/gm);
    expect(deployWorkflow).not.toContain('inventory.tsv');
    expect(deployWorkflow).not.toContain('LAMBDA_BINARIES');
    expect(deployWorkflow).not.toContain('secrets.CI_UPLOAD_ROLE_ARN');
  });

  test("initializes foundation, schema and FX before deploying compute/API", () => {
    expect(initializeWorkflow).not.toContain("dms_initial_cdc_start_position:");
    expect(initializeWorkflow).not.toContain("DMS_INITIAL_CDC_START_POSITION");
    expect(initializeWorkflow).not.toContain("DmsCdcInitialCdcStartPosition=");
    expect(initializeWorkflow).toContain("aws-deploy-${{ inputs.stage }}");
    expect(initializeWorkflow).toContain('for stack in network data initialize; do');
    expect(initializeWorkflow).toContain('if [ "$init_sha" != "$DEPLOY_COMMIT_SHA" ]; then');
    expect(initializeWorkflow).not.toMatch(/[A-Za-z]+Enabled=(?:true|false)/);
    expect(initializeWorkflow).toContain('"${STACK_NAME_PREFIX}-compute:PeriodicMatcherImageDigest=${PERIODIC_MATCHER_IMAGE_DIGEST}"');

    expect(initializeWorkflow).toContain('invoke_function "database-migration-lambda-${STAGE}" migration-invocation.json');
    expect(initializeWorkflow).toContain('invoke_function "fxrate-lambda-${STAGE}" fxrate-invocation.json');
    expect(initializeWorkflow).toContain("--cli-read-timeout 900");

    expect(initializeWorkflow).toContain('migration-result.json');
    expect(initializeWorkflow).toContain('fxrate-result.json');
    expect(initializeWorkflow).toContain("FunctionError");
    expect(initializeWorkflow).not.toContain("aws dms start-replication-task");
    expect(initializeWorkflow).not.toContain("NATIVE_PAUSED_AND_SETTLED");

    const foundationCheck = initializeWorkflow.indexOf('for stack in network data initialize; do');
    const computeDeploy = initializeWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-compute"');
    const migrationInvoke = initializeWorkflow.indexOf('invoke_function "database-migration-lambda-${STAGE}"');
    const fxInvoke = initializeWorkflow.indexOf('invoke_function "fxrate-lambda-${STAGE}"');
    const apiDeploy = initializeWorkflow.indexOf('stacks=("${STACK_NAME_PREFIX}-api")');
    expect(foundationCheck).toBeGreaterThanOrEqual(0);
    expect(migrationInvoke).toBeGreaterThan(foundationCheck);
    expect(fxInvoke).toBeGreaterThan(migrationInvoke);
    expect(computeDeploy).toBeGreaterThan(fxInvoke);
    expect(apiDeploy).toBeGreaterThan(computeDeploy);
  });
});
