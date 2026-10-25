"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  digest,
  bytesDigest,
  assertRequestBinding,
  assertDeployApproval,
  validateReport,
} = require("./contracts.cjs");
const { assertFullTests } = require("./gate.cjs");
const { validateObjects } = require("./build.cjs");
const state = require("./admission.cjs");
const inspection = require("./inspection.cjs");
const preparation = require("./preparation.cjs");
const {
  routineOwnership,
  verifyArtifacts,
  physicalTarget,
  verifyConsumerArtifacts,
  verifyCurrentHealth,
  healthInventory,
  smoke,
} = require("./verification.cjs");
const storage = require("./storage.cjs");
const { reviewContext } = require("./review-context.cjs");

function evidenceObject(reference, store = storage) {
  const uri = new URL(reference.uri);
  if (uri.protocol !== "s3:" || !uri.searchParams.get("versionId"))
    throw new Error("Pinned evidence object version required");
  const object = store.readObject(
    uri.hostname,
    decodeURIComponent(uri.pathname.slice(1)),
    uri.searchParams.get("versionId"),
  );
  if (bytesDigest(object.body) !== reference.sha256)
    throw new Error("Approved evidence object changed");
  return JSON.parse(object.body);
}
function loadApproved(env, options = {}) {
  const store = options.store || storage;
  const request = store.readJson(
    env.DELIVERY_BUCKET,
    env.DELIVERY_REQUEST_KEY,
    env.DELIVERY_REQUEST_VERSION,
  );
  if (
    digest(request) !== env.DELIVERY_REQUEST_DIGEST ||
    request.authorization.scope !== "DEPLOY" ||
    request.target !== env.DELIVERY_TARGET ||
    !env.DELIVERY_REQUEST_KEY.startsWith(`requests/${request.target}/`)
  )
    throw new Error("Exact approved request identity required");
  const tests = store.readJson(
    env.DELIVERY_BUCKET,
    `${env.DELIVERY_REQUEST_KEY.slice(0, -5)}/tests.json`,
  );
  assertFullTests(tests, request);
  const bundle = store.readJson(
    env.DELIVERY_BUCKET,
    `bundles/${env.DELIVERY_BUNDLE_DIGEST}.json`,
  );
  assertRequestBinding(request, tests, bundle);
  const report = store.readJson(
    env.DELIVERY_BUCKET,
    env.DELIVERY_REPORT_KEY,
    env.DELIVERY_REPORT_VERSION,
  );
  if (digest(report) !== env.DELIVERY_REPORT_DIGEST)
    throw new Error("Approved report identity changed");
  validateReport(report);
  const completion = store.readJson(
    env.DELIVERY_BUCKET,
    `requests/${request.target}/${request.id}-manual.json`,
  );
  assertDeployApproval(request, report, completion);
  for (const prerequisite of report.manualPrerequisites) {
    const observed = completion.observations?.find(
      (item) => item.prerequisiteId === prerequisite.id,
    );
    if (
      !observed?.mapping ||
      !observed.aliases ||
      digest({ mapping: observed.mapping, aliases: observed.aliases }) !==
        prerequisite.expectedAfterDigest
    )
      throw new Error(
        "Actual sanitized OpenSearch after-state evidence is missing or disagrees with approved prerequisite",
      );
  }
  const inventoryRefs = report.evidence.filter(
      (item) => item.kind === "INVENTORY",
    ),
    modelRefs = report.evidence.filter((item) => item.kind === "MODEL_REVIEW");
  if (inventoryRefs.length !== 1 || modelRefs.length !== 1)
    throw new Error(
      "Complete immutable inventory and reviewer evidence required",
    );
  const inventory = evidenceObject(inventoryRefs[0], store),
    model = evidenceObject(modelRefs[0], store);
  if (
    digest(model) !== report.reviewEvidenceDigest ||
    model.inputDigest !== digest(inventory) ||
    model.contextDigest !== digest(reviewContext(inventory)) ||
    model.contextDigest !== digest(model.context) ||
    inventory.hardFindings.length ||
    digest(inventory.config) !== report.configDigest ||
    inventory.database.ledger_fingerprint !== report.migrationLedgerDigest
  )
    throw new Error("Report/evidence/policy binding mismatch");
  const targetConfig = preparation.loadTargetConfig(request.target, {
    ...env,
    AWS_ACCOUNT_ID: report.account,
    DELIVERY_REVIEW_MODEL_ID: inventory.config.reviewer.modelId,
  });
  const ctx = preparation.context(bundle, targetConfig, env.DELIVERY_BUCKET);
  if (ctx.account !== report.account || ctx.region !== report.region)
    throw new Error("Wrong deployment account/region");
  const config = preparation.discover(
    ctx,
    targetConfig,
    options.api || state.aws,
  );
  if (digest(config) !== report.configDigest)
    throw new Error("Trusted target configuration changed since approval");
  if (!env.DELIVERY_EXECUTION_ID)
    throw new Error("Native execution ID required");
  return {
    ctx,
    config,
    request,
    tests,
    bundle,
    report,
    completion,
    inventory,
    execution: {
      executionId: env.DELIVERY_EXECUTION_ID,
      requestDigest: digest(request),
      bundleDigest: digest(bundle),
    },
  };
}
function assertOwned(store, approved) {
  const current = store.read()?.state;
  if (
    current?.state !== "IN_PROGRESS" ||
    digest(current.active) !== digest(approved.execution)
  )
    throw new Error(
      "Native execution does not own the environment admission hold",
    );
  return current;
}
function revalidatePlans(ctx, plans, config, api) {
  for (const plan of plans) {
    const expectedRole =
      plan.boundary === "migrate"
        ? config.migrationExecutionRoleArn
        : config.runtimeExecutionRoleArn;
    if (
      plan.changeSet.roleArn !== expectedRole ||
      plan.changeSet.templateDigest !== digest(plan.template)
    )
      throw new Error(
        "Approved change-set execution role/template binding differs from trusted target",
      );
    const value = api(ctx, "cloudformation", "describe-change-set", [
      "--change-set-name",
      plan.changeSet.id,
    ]);
    if (
      value.ChangeSetId !== plan.changeSet.id ||
      value.StackId !== plan.changeSet.stackId ||
      digest(value.Parameters || []) !== plan.changeSet.parametersDigest ||
      digest(value.Changes || []) !== digest(plan.changeSet.changes)
    )
      throw new Error("Approved native change set changed");
    if (plan.changeSet.status === "AVAILABLE") {
      const body = api(ctx, "cloudformation", "get-template", [
        "--stack-name",
        plan.changeSet.stackId,
        "--change-set-name",
        plan.changeSet.id,
        "--template-stage",
        "Original",
      ]).TemplateBody;
      if (
        digest(typeof body === "string" ? JSON.parse(body) : body) !==
        plan.changeSet.templateDigest
      )
        throw new Error(
          "Actual approved change-set template differs from reviewed immutable candidate",
        );
      if (
        value.Status !== "CREATE_COMPLETE" ||
        value.ExecutionStatus !== "AVAILABLE"
      )
        throw new Error("Approved native change set is no longer executable");
    } else if (
      value.Status !== "FAILED" ||
      !/didn.t contain changes|No updates are to be performed/.test(
        value.StatusReason || "",
      )
    )
      throw new Error("Native no-change evidence changed");
  }
}
async function admit(env = process.env, options = {}) {
  const api = options.api || state.aws,
    store = options.store || storage,
    approved = loadApproved(env, { ...options, api, store });
  validateObjects(approved.bundle, store);
  const status = options.stateStore || state.stateStore(approved.ctx, api);
  try {
    const current = status.read()?.state,
      baseline = approved.inventory.baselineState;
    const ownRetry =
      current?.state === "IN_PROGRESS" &&
      digest(current.active) === digest(approved.execution);
    if (
      !current ||
      (!ownRetry && digest(current) !== digest(baseline)) ||
      digest(current.lastVerified) !== digest(baseline.lastVerified) ||
      digest(current.observedSchema) !== digest(baseline.observedSchema)
    )
      throw new Error(
        "Target baseline changed; new report and approval required",
      );
    const database = inspection.inspectDatabase(
      approved.ctx,
      approved.config.inspectorFunctionArn,
      api,
    );
    const live = inspection.collectAws(
      approved.ctx,
      approved.config.stackNames,
      approved.config.dmsTaskArn,
      api,
    );
    if (
      live.health.dmsStatus !== approved.inventory.dmsBaseline.status ||
      !["running", "stopped", "ready"].includes(live.health.dmsStatus) ||
      live.health.tablesErrored !== 0
    )
      throw new Error(
        "Approved capture lifecycle/health changed before admission",
      );
    if (
      database.ledger_fingerprint !== approved.report.migrationLedgerDigest ||
      digest(database.schema) !== digest(approved.inventory.database.schema) ||
      inspection.identity(live) !== inspection.identity(approved.inventory.aws)
    )
      throw new Error(
        "Database/configuration/artifact baseline drifted after approval",
      );
    revalidatePlans(
      approved.ctx,
      approved.inventory.plans,
      approved.config,
      api,
    );
    if (!ownRetry) state.acquire(status, baseline, approved.execution);
    fs.mkdirSync("prepared", { recursive: true });
    const variables = {};
    for (const plan of approved.inventory.plans) {
      const template = store.readObject(
        plan.artifact.bucket,
        plan.artifact.key,
        plan.artifact.versionId,
      );
      if (
        template.versionId !== plan.artifact.versionId ||
        bytesDigest(template.body) !== plan.artifact.sha256
      )
        throw new Error("Prepared template differs from approved immutable artifact");
      fs.writeFileSync(
        `prepared/${plan.boundary}.template.json`,
        template.body,
      );
      fs.writeFileSync(
        `prepared/${plan.boundary}.config.json`,
        JSON.stringify({ Parameters: plan.parameters }),
      );
      const stem = plan.boundary[0].toUpperCase() + plan.boundary.slice(1);
      variables[`${stem}ActionMode`] =
        plan.changeSet.status === "AVAILABLE"
          ? "CHANGE_SET_EXECUTE"
          : "CREATE_UPDATE";
      variables[`${stem}ChangeSetName`] = plan.changeSet.name;
    }
    const admissionKey = `reports/executions/${approved.execution.executionId}/admission.json`;
    const existingAdmission = store
      .aws("s3api", "list-objects-v2", [
        "--bucket",
        approved.ctx.bucket,
        "--prefix",
        admissionKey,
      ])
      .Contents?.some((object) => object.Key === admissionKey);
    const evidence = existingAdmission
      ? store.readJson(approved.ctx.bucket, admissionKey)
      : {
          version: 1,
          execution: approved.execution,
          reportDigest: digest(approved.report),
          startedAt: new Date().toISOString(),
        };
    if (
      digest(evidence.execution) !== digest(approved.execution) ||
      evidence.reportDigest !== digest(approved.report)
    )
      throw new Error(
        "Existing native admission evidence disagrees with this execution",
      );
    fs.writeFileSync("prepared/execution.json", JSON.stringify(evidence));
    fs.writeFileSync(
      "prepared/action-variables.json",
      JSON.stringify(variables),
    );
    store.putImmutableJson(
      approved.ctx.bucket,
      `reports/executions/${approved.execution.executionId}/admission.json`,
      evidence,
    );
    return evidence;
  } catch (error) {
    const current = status.read()?.state;
    if (
      current?.state === "IN_PROGRESS" &&
      digest(current.active) === digest(approved.execution)
    )
      state.fail(status, approved.execution, error.message);
    throw error;
  } finally {
    status.close?.();
  }
}
function preparedExecution(env, approved) {
  const directory =
    env.DELIVERY_PREPARED_DIR ||
    path.join(env.CODEBUILD_SRC_DIR_Prepared || ".", "prepared");
  const evidence = JSON.parse(
    fs.readFileSync(path.join(directory, "execution.json")),
  );
  if (
    digest(evidence.execution) !== digest(approved.execution) ||
    evidence.reportDigest !== digest(approved.report)
  )
    throw new Error(
      "Prepared artifact belongs to a different native execution or approval",
    );
  return evidence;
}
function invokeMigrator(ctx, functionArn, request, api = state.aws) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "aura-migrate-"));
  try {
    const input = path.join(directory, "input.json"),
      output = path.join(directory, "output.json");
    fs.writeFileSync(input, JSON.stringify(request), { mode: 0o600 });
    const response = api(
      ctx,
      "lambda",
      "invoke",
      [
        "--function-name",
        functionArn,
        "--invocation-type",
        "RequestResponse",
        "--cli-binary-format",
        "raw-in-base64-out",
        "--payload",
        `fileb://${input}`,
        output,
      ],
      { timeout: 910000, readTimeoutSeconds: 900 },
    );
    if (
      response.FunctionError ||
      response.StatusCode !== 200 ||
      response.ExecutedVersion !== functionArn.split(":").at(-1)
    )
      throw new Error(
        "Candidate migration outcome failed or unknown; inspect SQLx ledger before continuing",
      );
    return JSON.parse(fs.readFileSync(output));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}
async function migrate(env = process.env, options = {}) {
  const api = options.api || state.aws,
    store = options.store || storage,
    approved = loadApproved(env, { ...options, api, store });
  preparedExecution(env, approved);
  const status = options.stateStore || state.stateStore(approved.ctx, api);
  try {
    assertOwned(status, approved);
    const functionArn = preparation.output(
      approved.ctx,
      `application-${approved.ctx.target}-migrate`,
      "DatabaseMigrationVersionArn",
      api,
    );
    if (
      !new RegExp(
        `^arn:aws:lambda:${approved.ctx.region}:${approved.ctx.account}:function:database-migration-lambda-${approved.ctx.target}:[1-9][0-9]*$`,
      ).test(functionArn)
    )
      throw new Error("Exact deployed candidate migration version required");
    const identity = api(approved.ctx, "lambda", "get-function-configuration", [
      "--function-name",
      functionArn,
    ]);
    const artifact = approved.bundle.artifacts.find(
      (item) =>
        item.kind === "LAMBDA" && item.id === "database-migration-lambda",
    );
    if (
      !artifact ||
      identity.CodeSha256 !==
        Buffer.from(artifact.sha256, "hex").toString("base64") ||
      identity.Version !== functionArn.split(":").at(-1)
    )
      throw new Error(
        "Deployed candidate migrator bytes differ from approved bundle",
      );
    const known = new Set(
      approved.inventory.database.ledger.map((row) => row.version),
    );
    const request = {
      request_id: approved.request.id,
      bundle_digest: digest(approved.bundle),
      expected_ledger_fingerprint: approved.report.migrationLedgerDigest,
      pending: approved.inventory.source.migrations
        .filter((item) => !known.has(item.version))
        .map((item) => ({ version: item.version, checksum: item.checksum })),
    };
    const result = (options.invokeMigrator || invokeMigrator)(
      approved.ctx,
      functionArn,
      request,
      api,
    );
    if (
      !["APPLIED", "NO_OP"].includes(result.status) ||
      result.request_id !== request.request_id ||
      result.bundle_digest !== request.bundle_digest ||
      result.target !== approved.ctx.target ||
      result.ledger_fingerprint !== digest(result.ledger) ||
      inspection.ledgerFindings(
        result.ledger,
        approved.inventory.source.migrations,
      ).length ||
      result.ledger.length !== approved.inventory.source.migrations.length
    )
      throw new Error(
        "Candidate migrator did not prove exact reviewed schema success",
      );
    const evidence = {
      version: 1,
      execution: approved.execution,
      functionArn,
      request,
      result,
    };
    store.putImmutableJson(
      approved.ctx.bucket,
      `reports/executions/${approved.execution.executionId}/migration-result.json`,
      evidence,
    );
    return evidence;
  } catch (error) {
    state.fail(status, approved.execution, error.message);
    throw error;
  } finally {
    status.close?.();
  }
}
async function verify(env = process.env, options = {}) {
  const api = options.api || state.aws,
    store = options.store || storage,
    approved = loadApproved(env, { ...options, api, store }),
    prepared = preparedExecution(env, approved);
  const status = options.stateStore || state.stateStore(approved.ctx, api);
  try {
    assertOwned(status, approved);
    const migration = store.readJson(
      approved.ctx.bucket,
      `reports/executions/${approved.execution.executionId}/migration-result.json`,
    );
    if (digest(migration.execution) !== digest(approved.execution))
      throw new Error("Exact candidate migration evidence missing");
    const database = inspection.inspectDatabase(
      approved.ctx,
      approved.config.inspectorFunctionArn,
      api,
    );
    if (
      inspection.ledgerFindings(
        database.ledger,
        approved.inventory.source.migrations,
      ).length ||
      database.ledger.length !== approved.inventory.source.migrations.length ||
      digest(database.schema) !== digest(approved.inventory.schema.after)
    )
      throw new Error("Actual final schema differs from reviewed candidate");
    const live = inspection.collectAws(
      approved.ctx,
      approved.config.stackNames,
      approved.config.dmsTaskArn,
      api,
    );
    const owned = routineOwnership(approved.inventory.plans, live);
    const protectedFunctions = (functions) =>
      functions.filter(
        (item) => !owned.functions.includes(item.arn.split(":")[6]),
      );
    if (
      digest(protectedFunctions(live.functions)) !==
      digest(protectedFunctions(approved.inventory.aws.functions))
    )
      throw new Error(
        "Protected operational function code/configuration identity changed",
      );
    if (
      digest(live.capture) !== digest(approved.inventory.aws.capture) ||
      digest(
        inspection.normalizedActivation(
          live.activation,
          owned.functions,
          owned.tasks,
        ),
      ) !==
        digest(
          inspection.normalizedActivation(
            approved.inventory.aws.activation,
            owned.functions,
            owned.tasks,
          ),
        )
    )
      throw new Error(
        "Capture or intentional consumer activation identity changed",
      );
    for (const [name, before] of Object.entries(approved.inventory.aws.stacks))
      if (
        !approved.inventory.plans.some((plan) => plan.stackName === name) &&
        digest(live.stacks[name]) !== digest(before)
      )
        throw new Error(
          "Protected platform stack changed during routine delivery",
        );
    verifyArtifacts(
      approved.ctx,
      approved.bundle,
      approved.inventory.plans,
      live,
      api,
    );
    verifyConsumerArtifacts(
      approved.ctx,
      approved.bundle,
      approved.inventory.plans,
      live,
      api,
    );
    const smokeEvidence = await (options.smoke || smoke)(
      approved.config,
      options.fetch,
    );
    const healthSince = new Date(
      Math.max(
        Date.parse(prepared.startedAt),
        ...approved.inventory.plans.map((plan) =>
          Date.parse(live.stacks[plan.stackName].updatedAt),
        ),
      ),
    ).toISOString();
    const health = await verifyCurrentHealth(
      approved.ctx,
      approved.config,
      approved.inventory.dmsBaseline,
      healthSince,
      {
        api,
        live,
        requirePass: false,
        fetch: options.fetch,
        routerBaseline: approved.inventory.aws.activation.find((item) =>
          item.source?.startsWith("arn:aws:kinesis:"),
        ),
      },
    );
    const evidence = {
      version: 1,
      execution: approved.execution,
      sourceSha: approved.bundle.sourceSha,
      bundleDigest: digest(approved.bundle),
      observedAt: new Date().toISOString(),
      database,
      smoke: smokeEvidence,
      health,
    };
    store.putImmutableJson(
      approved.ctx.bucket,
      `reports/executions/${approved.execution.executionId}/verification/${digest(evidence)}.json`,
      evidence,
    );
    if (health.status !== "PASS")
      throw new Error(
        "Bounded CDC/worker health remains failed or unverified; environment hold retained",
      );
    state.finalize(status, approved.execution, {
      verified: true,
      nativeExecutionId: approved.execution.executionId,
      bundleDigest: digest(approved.bundle),
      sourceSha: approved.bundle.sourceSha,
      verifiedAt: evidence.observedAt,
      observedSchema: {
        sha256: digest(database.schema),
        observedAt: evidence.observedAt,
        migrations: database.ledger.map((row) => ({
          version: String(row.version),
          checksum: row.checksum,
        })),
      },
    });
    return evidence;
  } catch (error) {
    state.fail(status, approved.execution, error.message);
    throw error;
  } finally {
    status.close?.();
  }
}
module.exports = {
  evidenceObject,
  loadApproved,
  assertOwned,
  revalidatePlans,
  admit,
  preparedExecution,
  invokeMigrator,
  migrate,
  verifyArtifacts,
  physicalTarget,
  verifyConsumerArtifacts,
  verifyCurrentHealth,
  healthInventory,
  smoke,
  verify,
};
