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
    expect(deployWorkflow).toContain("crate: src/aura-historia-worker");
    expect(deployWorkflow).toContain("binary: cdc-router-lambda");
    expect(deployWorkflow).toContain("--bin \"${{ matrix.binary }}\"");
    expect(deployWorkflow).toContain("target/lambda/$BIN_NAME/bootstrap.zip");
    expect(deployWorkflow).toContain("ref: ${{ env.DEPLOY_COMMIT_SHA }}");
    expect(deployWorkflow).toContain("DEPLOY_COMMIT_SHA: ${{ github.event_name == 'workflow_dispatch' && inputs.commit_sha || github.event_name == 'push' && github.sha || '' }}");
    expect(deployWorkflow).toContain("Data foundation is not initialized; run Initialize (CD) first.");
    expect(deployWorkflow).toContain("Initialization runtime is not deployed; run Initialize (CD) first.");
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).toContain('"${STACK_NAME_PREFIX}-compute:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(deployWorkflow).not.toContain("deployment_phase:");
    expect(deployWorkflow).not.toContain("dms_initial_cdc_start_position:");
    expect(deployWorkflow).not.toContain("ProductListingOpenSearchConsumerEnabled=");
  });

  test("updates foundation, schema, FX, and consumer activation only from manual initialize", () => {
    expect(initializeWorkflow).not.toContain("dms_initial_cdc_start_position:");
    expect(initializeWorkflow).not.toContain("DMS_INITIAL_CDC_START_POSITION");
    expect(initializeWorkflow).not.toContain("DmsCdcInitialCdcStartPosition=");
    expect(initializeWorkflow).toContain("aws-deploy-${{ inputs.stage }}");
    expect(initializeWorkflow).toContain('deploy "${STACK_NAME_PREFIX}-network"');
    expect(initializeWorkflow).toContain('deploy "${STACK_NAME_PREFIX}-data"');
    expect(initializeWorkflow).toContain('"${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"');
    expect(initializeWorkflow).toContain('ProductListingOpenSearchConsumerEnabled=false');
    expect(initializeWorkflow).toContain('ProductListingOpenSearchConsumerEnabled=true');
    expect(initializeWorkflow).toContain('invoke_function "database-migration-lambda-${STAGE}" migration-invocation.json');
    expect(initializeWorkflow).toContain('invoke_function "fxrate-lambda-${STAGE}" fxrate-invocation.json');
    expect(initializeWorkflow).toContain("--cli-read-timeout 900");
    expect(initializeWorkflow).toContain("FunctionError");
    expect(initializeWorkflow).not.toContain("aws dms start-replication-task");
    expect(initializeWorkflow).not.toContain("NATIVE_PAUSED_AND_SETTLED");

    const networkDeploy = initializeWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-network"');
    const dataDeploy = initializeWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-data"');
    const initializationDeploy = initializeWorkflow.indexOf('deploy "${STACK_NAME_PREFIX}-initialize"');
    const inactiveCompute = initializeWorkflow.indexOf('ProductListingOpenSearchConsumerEnabled=false');
    const migrationInvoke = initializeWorkflow.indexOf('invoke_function "database-migration-lambda-${STAGE}"');
    const fxInvoke = initializeWorkflow.indexOf('invoke_function "fxrate-lambda-${STAGE}"');
    const activeCompute = initializeWorkflow.lastIndexOf('ProductListingOpenSearchConsumerEnabled=true');
    expect(networkDeploy).toBeGreaterThanOrEqual(0);
    expect(dataDeploy).toBeGreaterThan(networkDeploy);
    expect(initializationDeploy).toBeGreaterThan(dataDeploy);
    expect(inactiveCompute).toBeGreaterThan(initializationDeploy);
    expect(migrationInvoke).toBeGreaterThan(inactiveCompute);
    expect(fxInvoke).toBeGreaterThan(migrationInvoke);
    expect(activeCompute).toBeGreaterThan(fxInvoke);
  });
});
