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
    expect(deployWorkflow).toContain('needs: [infra-test, aws-push-lambda, aws-push-mail-templates]');
    expect(deployWorkflow).toContain("needs.infra-test.result == 'success'");
    expect(deployWorkflow).toContain("needs.aws-push-lambda.result == 'success'");
    expect(deployWorkflow).toContain("needs.aws-push-mail-templates.result == 'success'");
    expect(deployWorkflow).toContain('secrets.CI_DEPLOY_ROLE_ARN');
    expect(deployWorkflow).toContain('migration-result.json');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-compute:CommitSHA=${DEPLOY_COMMIT_SHA}"');
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
