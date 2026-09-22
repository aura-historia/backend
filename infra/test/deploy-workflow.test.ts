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

describe("release workflow boundary", () => {
  test("keeps pushes artifact-only and normal deploy limited to stage plus immutable artifact source", () => {
    expect(deployWorkflow).toContain("github.event_name == 'workflow_dispatch'");
    expect(deployWorkflow).toContain("cancel-in-progress: false");
    expect(deployWorkflow).toContain("stage:");
    expect(deployWorkflow).toContain("commit_sha:");
    expect(deployWorkflow).toContain("database-migration-lambda");
    expect(deployWorkflow).toContain("Data foundation is not initialized; run Initialize (CD) first.");
    expect(deployWorkflow).toContain("Initialization runtime is not deployed; run Initialize (CD) first.");
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-compute:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).not.toContain("deployment_phase:");
    expect(deployWorkflow).not.toContain("dms_initial_cdc_start_position:");
    expect(deployWorkflow).not.toContain("ProductListingOpenSearchConsumerEnabled=");
  });

  test("runs first-time infrastructure, schema, FX, and consumer activation only from manual initialize", () => {
    expect(initializeWorkflow).toContain("dms_initial_cdc_start_position:");
    expect(initializeWorkflow).toContain("dms_initial_cdc_start_position must be an approved uppercase PostgreSQL LSN");
    expect(initializeWorkflow).toContain("aws-deploy-${{ inputs.stage }}");
    expect(initializeWorkflow).toContain('"${STACK_NAME_PREFIX}-data:DmsCdcInitialCdcStartPosition=${DMS_INITIAL_CDC_START_POSITION}"');
    expect(initializeWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(initializeWorkflow).toContain('ProductListingOpenSearchConsumerEnabled=false');
    expect(initializeWorkflow).toContain('ProductListingOpenSearchConsumerEnabled=true');
    expect(initializeWorkflow).toContain('invoke_function "database-migration-lambda-${STAGE}" migration-invocation.json');
    expect(initializeWorkflow).toContain('invoke_function "fxrate-lambda-${STAGE}" fxrate-invocation.json');
    expect(initializeWorkflow).toContain("--cli-read-timeout 900");
    expect(initializeWorkflow).toContain("FunctionError");
    expect(initializeWorkflow).not.toContain("NATIVE_PAUSED_AND_SETTLED");

    const initializationDeploy = initializeWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-initialize"');
    const inactiveCompute = initializeWorkflow.indexOf('ProductListingOpenSearchConsumerEnabled=false');
    const migrationInvoke = initializeWorkflow.indexOf('invoke_function "database-migration-lambda-${STAGE}"');
    const fxInvoke = initializeWorkflow.indexOf('invoke_function "fxrate-lambda-${STAGE}"');
    const activeCompute = initializeWorkflow.lastIndexOf('ProductListingOpenSearchConsumerEnabled=true');
    expect(initializationDeploy).toBeGreaterThanOrEqual(0);
    expect(inactiveCompute).toBeGreaterThan(initializationDeploy);
    expect(migrationInvoke).toBeGreaterThan(inactiveCompute);
    expect(fxInvoke).toBeGreaterThan(migrationInvoke);
    expect(activeCompute).toBeGreaterThan(fxInvoke);
  });
});
