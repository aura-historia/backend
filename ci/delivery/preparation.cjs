"use strict";
const fs = require("node:fs");
const path = require("node:path");
const {
  digest,
  bytesDigest,
  validateBundle,
  validateReport,
} = require("./contracts.cjs");
const { assertFullTests } = require("./gate.cjs");
const { validateObjects } = require("./build.cjs");
const admission = require("./admission.cjs");
const inspection = require("./inspection.cjs");
const schema = require("./schema.cjs");
const reviewer = require("./review.cjs");
const reviewObservations = require("./review-observations.cjs");
const storage = require("./storage.cjs");
const imageCatalog = require("../container-images.json");

function loadTargetConfig(target, env = process.env) {
  if (!["dev", "prod"].includes(target))
    throw new Error("Explicit delivery target required");
  const filename =
    env.DELIVERY_TARGET_CONFIG ||
    path.join(__dirname, "targets", `${target}.json`);
  let text = fs.readFileSync(filename, "utf8");
  text = text.replace(
    /\$\{(AWS_ACCOUNT_ID|DELIVERY_REVIEW_MODEL_ID)\}/g,
    (_match, key) => env[key] || "UNCONFIGURED",
  );
  const config = JSON.parse(text);
  if (
    config.version !== 1 ||
    config.target !== target ||
    !/^\d{12}$/.test(config.account || "") ||
    config.region !== "eu-central-1" ||
    !Array.isArray(config.stackNames) ||
    !config.stackNames.length ||
    new Set(config.stackNames).size !== config.stackNames.length ||
    config.stackNames.some(
      (name) =>
        !new RegExp(
          `^application-${target}-(network|data|identity|initialize|migrate|compute|api|observability)$`,
        ).test(name),
    )
  )
    throw new Error("Trusted target configuration is incomplete");
  return config;
}
function context(bundle, config, bucket) {
  const selected = bundle.targets[config.target];
  if (
    !selected ||
    selected.account !== config.account ||
    selected.region !== config.region ||
    !bucket
  )
    throw new Error(
      "Bundle/target/account/region configuration mismatch or disabled target",
    );
  return {
    target: config.target,
    account: config.account,
    region: config.region,
    bucket,
  };
}
function output(ctx, stackName, key, api = admission.aws) {
  const stacks = api(ctx, "cloudformation", "describe-stacks", [
    "--stack-name",
    stackName,
  ]).Stacks;
  const values =
    stacks?.length === 1 &&
    stacks[0].Outputs?.filter((item) => item.OutputKey === key);
  if (!values || values.length !== 1 || !values[0].OutputValue)
    throw new Error(`Trusted operational output ${key} missing`);
  return values[0].OutputValue;
}
function discover(ctx, config, api = admission.aws) {
  const resources = api(ctx, "cloudformation", "list-stack-resources", [
    "--stack-name",
    `application-${ctx.target}-data`,
  ]).StackResourceSummaries;
  const tasks = resources?.filter(
    (item) => item.ResourceType === "AWS::DMS::ReplicationTask",
  );
  if (tasks?.length !== 1)
    throw new Error("Exactly one protected CDC task required");
  return {
    ...config,
    dmsTaskArn: config.dmsTaskArn || tasks[0].PhysicalResourceId,
    inspectorFunctionArn:
      config.inspectorFunctionArn ||
      output(
        ctx,
        `application-${ctx.target}-initialize`,
        "DatabaseInspectionVersionArn",
        api,
      ),
  };
}
function artifactParameters(bundle) {
  const values = {
    CommitSHA: bundle.sourceSha,
    MailTemplatePrefix: `mail/${bundle.build.id}/`,
  };
  for (const asset of bundle.artifacts.filter(
    (item) => item.kind === "LAMBDA",
  )) {
    const stem = asset.id
      .split("-")
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join("");
    values[`${stem}ArtifactKey`] = asset.key;
    values[`${stem}ArtifactVersion`] = asset.versionId;
    values[`${stem}ArtifactSha256`] = asset.sha256;
  }
  for (const image of bundle.images) {
    const catalog = imageCatalog.find((item) => item.id === image.id);
    if (!catalog)
      throw new Error("Image has no trusted runtime parameter catalog");
    values[catalog.digestParameter] = image.digest;
  }
  return values;
}
function effectiveParameters(template, live, values) {
  const existing = new Map(
    (live.Parameters || []).map((item) => [
      item.ParameterKey,
      item.ParameterValue,
    ]),
  );
  const changeset = [],
    native = {};
  for (const [key, definition] of Object.entries(template.Parameters || {})) {
    if (Object.hasOwn(values, key)) {
      changeset.push({
        ParameterKey: key,
        ParameterValue: String(values[key]),
      });
      native[key] = String(values[key]);
    } else if (existing.has(key)) {
      changeset.push({ ParameterKey: key, UsePreviousValue: true });
      if (key.endsWith("Enabled")) {
        if (!["true", "false"].includes(existing.get(key)))
          throw new Error("Invalid intentional activation state");
        native[key] = existing.get(key);
      }
    } else if (Object.hasOwn(definition, "Default"))
      changeset.push({
        ParameterKey: key,
        ParameterValue: String(definition.Default),
      });
    else
      throw new Error(
        `New unbound stack parameter ${key} requires maintenance`,
      );
  }
  return { changeset, native };
}
function candidateTemplates(bundle, target, store = storage) {
  const templates = {};
  for (const asset of bundle.artifacts.filter(
    (item) =>
      item.kind === "ASSET" &&
      item.id.startsWith(`template-${target}-`) &&
      item.id.endsWith(".template.json"),
  )) {
    const object = store.readObject(asset.bucket, asset.key, asset.versionId);
    if (bytesDigest(object.body) !== asset.sha256)
      throw new Error("Candidate template object changed");
    const name = asset.id.slice(
      `template-${target}-`.length,
      -".template.json".length,
    );
    if (templates[name]) throw new Error("Duplicate candidate stack template");
    templates[name] = { template: JSON.parse(object.body), artifact: asset };
  }
  return templates;
}
function capturedRelations(task) {
  const mappings = JSON.parse(task.TableMappings);
  if (!Array.isArray(mappings.rules))
    throw new Error("Missing capture selection rules");
  const selected = mappings.rules.filter(
    (rule) => rule["rule-type"] === "selection",
  );
  if (
    !selected.length ||
    selected.some(
      (rule) =>
        rule["rule-action"] !== "include" ||
        !/^[a-z_][a-z0-9_]*$/.test(
          rule["object-locator"]?.["schema-name"] || "",
        ) ||
        !/^[a-z_][a-z0-9_]*$/.test(
          rule["object-locator"]?.["table-name"] || "",
        ) ||
        rule.filters?.length,
    )
  )
    throw new Error("Unsupported capture selections require maintenance");
  return selected
    .map(
      (rule) =>
        `${rule["object-locator"]["schema-name"]}.${rule["object-locator"]["table-name"]}`,
    )
    .sort();
}
function readTask(ctx, arn, api = admission.aws) {
  const result = api(ctx, "dms", "describe-replication-tasks", [
    "--filters",
    `Name=replication-task-arn,Values=${arn}`,
  ]).ReplicationTasks;
  if (result?.length !== 1 || result[0].ReplicationTaskArn !== arn)
    throw new Error("Incomplete actual DMS task");
  return result[0];
}
function readEvidence(config, store = storage, supplied = null) {
  if (supplied) {
    const allowed = [
      "target",
      "targetHost",
      "index",
      "alias",
      "mapping",
      "aliases",
      "observedAt",
    ];
    if (
      Object.keys(supplied).some((key) => !allowed.includes(key)) ||
      supplied.target !== config.target ||
      !supplied.targetHost ||
      !supplied.index ||
      !supplied.mapping ||
      !supplied.aliases ||
      !Number.isFinite(Date.parse(supplied.observedAt)) ||
      Date.now() - Date.parse(supplied.observedAt) > 86400000 ||
      Date.parse(supplied.observedAt) > Date.now() + 60000
    )
      throw new Error(
        "Sanitized target-bound fresh OpenSearch before observation required",
      );
    return supplied;
  }
  if (!config.searchEvidence) return null;
  const reference = config.searchEvidence;
  const object = store.readObject(
    reference.bucket,
    reference.key,
    reference.versionId,
  );
  if (bytesDigest(object.body) !== reference.sha256)
    throw new Error("Sanitized OpenSearch observation changed");
  const value = JSON.parse(object.body);
  if (
    value.target !== config.target ||
    !value.targetHost ||
    !value.index ||
    !value.mapping ||
    !value.aliases ||
    !Number.isFinite(Date.parse(value.observedAt))
  )
    throw new Error("Incomplete private OpenSearch observation");
  return value;
}
function searchPrerequisites(ctx, bundle, source, search) {
  const findings = [],
    manual = [],
    patches = [];
  const changed = source.context.filter((item) =>
    item.path.startsWith("opensearch/"),
  );
  if (!changed.length) return { findings, manual, patches };
  if (!search)
    return {
      findings: [
        {
          code: "MISSING_PRIVATE_SEARCH_EVIDENCE",
          path: "opensearch",
          detail:
            "Provide sanitized actual mapping/alias observations before approval",
        },
      ],
      manual,
      patches,
    };
  for (const asset of changed) {
    if (
      asset.path !== "opensearch/mappings/product_listings.json" ||
      !asset.after
    ) {
      findings.push({
        code: "SEARCH_ADMIN_OR_NONADDITIVE_ASSET_CHANGE",
        path: asset.path,
        detail:
          "Host/security/pipeline/engine changes require separately reviewed maintenance",
      });
      continue;
    }
    const desired = JSON.parse(asset.after).mappings;
    try {
      const generated = reviewer.openSearchPrerequisite(
        ctx.target,
        digest(bundle),
        {
          targetHost: search.targetHost,
          index: search.index,
          alias: search.alias || search.index,
          before: search.mapping,
          after: desired,
          beforeAliases: search.aliases,
          afterAliases: search.aliases,
        },
        bundle.sourceAssets.filter((item) => item.path === asset.path),
      );
      manual.push(generated.prerequisite);
      patches.push({ name: generated.patchName, content: generated.patch });
    } catch (error) {
      findings.push({
        code: "INCOMPATIBLE_SEARCH_TRANSITION",
        path: asset.path,
        detail: error.message,
      });
    }
  }
  return { findings, manual, patches };
}
async function prepare(env = process.env, options = {}) {
  const store = options.store || storage,
    api = options.api || admission.aws;
  const bundle = store.readJson(
    env.DELIVERY_BUCKET,
    `bundles/${env.DELIVERY_BUNDLE_DIGEST}.json`,
  );
  validateBundle(bundle);
  if (
    digest(bundle) !== env.DELIVERY_BUNDLE_DIGEST ||
    !/^[a-f0-9]{64}$/.test(env.DELIVERY_REQUEST_ID || "")
  )
    throw new Error("Exact immutable candidate/request required");
  validateObjects(bundle, store);
  const tests = JSON.parse(
    fs.readFileSync(env.DELIVERY_TESTS_FILE || ".delivery/tests.json"),
  );
  assertFullTests(tests, {
    sourceSha: bundle.sourceSha,
    workflowRevision: env.WORKFLOW_REVISION,
    invocationRevision: env.INVOCATION_REVISION,
    runId: env.GITHUB_RUN_ID,
    runAttempt: Number(env.GITHUB_RUN_ATTEMPT),
  });
  if (
    !env.WORKFLOW_REVISION ||
    !env.INVOCATION_REVISION ||
    !env.GITHUB_RUN_ID ||
    !Number.isInteger(Number(env.GITHUB_RUN_ATTEMPT))
  )
    throw new Error("Trusted exact workflow test binding required");
  const config = loadTargetConfig(env.DELIVERY_TARGET, env),
    ctx = context(bundle, config, env.DELIVERY_BUCKET);
  const operational = discover(ctx, config, api),
    state = options.stateStore || admission.stateStore(ctx, api);
  try {
    const baseline = state.read()?.state;
    if (!baseline?.lastVerified || baseline.state !== "READY")
      throw new Error(
        "Unknown or held target baseline requires explicit operator reconciliation/adoption",
      );
    const source = inspection.cumulativeSource(
      env.DELIVERY_REPO_DIR || ".",
      baseline.lastVerified.sourceSha,
      bundle.sourceSha,
    );
    if (
      digest(
        source.migrations.map((item) => ({
          version: String(item.version),
          checksum: item.checksum,
        })),
      ) !== digest(bundle.migrations)
    )
      throw new Error(
        "Source history differs from immutable candidate migration inventory",
      );
    const database = inspection.inspectDatabase(
      ctx,
      operational.inspectorFunctionArn,
      api,
    );
    const live = inspection.collectAws(
      ctx,
      operational.stackNames,
      operational.dmsTaskArn,
      api,
    );
    const findings = [
      ...inspection.ledgerFindings(database.ledger, source.migrations),
    ];
    let snapshots;
    try {
      snapshots = await (options.snapshots || schema.snapshots)(
        source.migrations,
        database.ledger,
      );
    } catch {
      snapshots = { before: database.schema, after: null };
      findings.push({
        code: "SCHEMA_RECONSTRUCTION_FAILED",
        path: "migrations",
        detail:
          "Actual history cannot produce a complete isolated candidate schema; human review required",
      });
    }
    const task = readTask(ctx, operational.dmsTaskArn, api);
    for (const migration of source.migrations.filter(
      (item) => !database.ledger.some((row) => row.version === item.version),
    ))
      if (migration.sql.startsWith("-- no-transaction"))
        findings.push({
          code: "NONTRANSACTIONAL_MIGRATION",
          path: migration.path,
          detail:
            "Routine Lambda migration budget requires reviewed transactional forward SQL",
        });
    if (digest(snapshots.before) !== digest(database.schema))
      findings.push({
        code: "ACTUAL_SCHEMA_DIFFERS_FROM_HISTORY",
        path: "database/schema",
        detail:
          "Actual schema cannot be reconstructed from its authoritative migration ledger",
      });
    findings.push(
      ...inspection.schemaFindings(
        snapshots.before,
        snapshots.after,
        capturedRelations(task),
      ),
    );
    if (
      !["running", "stopped", "ready"].includes(task.Status) ||
      task.ReplicationTaskStats?.TablesErrored !== 0
    )
      findings.push({
        code: "CDC_HEALTH_UNVERIFIED",
        path: operational.dmsTaskArn,
        detail:
          "Routine delivery observes existing lifecycle without repairing it",
      });
    const templates = candidateTemplates(bundle, ctx.target, store);
    for (const name of Object.keys(templates))
      if (!operational.stackNames.includes(name))
        findings.push({
          code: "UNREGISTERED_STACK_TEMPLATE",
          path: name,
          detail: "New stack ownership is separately approved platform work",
        });
    const values = artifactParameters(bundle),
      plans = [];
    for (const name of operational.stackNames) {
      const desired = templates[name],
        before = live.stacks[name];
      if (!desired) throw new Error(`Candidate omits target stack ${name}`);
      if (
        !new RegExp(
          `^application-${ctx.target}-(migrate|compute|api|observability)$`,
        ).test(name)
      ) {
        if (digest(before.template) !== digest(desired.template))
          findings.push({
            code: "PROTECTED_STACK_TEMPLATE_CHANGED",
            path: name,
            detail:
              "Platform/identity/inspector/bootstrap changes require separate infrastructure approval",
          });
        continue;
      }
      const actual = api(ctx, "cloudformation", "describe-stacks", [
        "--stack-name",
        name,
      ]).Stacks[0];
      const parameters = effectiveParameters(desired.template, actual, values);
      const artifact = desired.artifact;
      const templateUrl = `https://s3.${ctx.region}.amazonaws.com/${artifact.bucket}/${artifact.key}?versionId=${encodeURIComponent(artifact.versionId)}`;
      const roleArn = name.endsWith("-migrate")
        ? env.DELIVERY_MIGRATION_EXECUTION_ROLE_ARN ||
          operational.migrationExecutionRoleArn
        : env.DELIVERY_EXECUTION_ROLE_ARN ||
          operational.runtimeExecutionRoleArn;
      if (
        !roleArn ||
        roleArn !==
          (name.endsWith("-migrate")
            ? operational.migrationExecutionRoleArn
            : operational.runtimeExecutionRoleArn)
      )
        throw new Error(
          "Separately provisioned CloudFormation execution role required",
        );
      const changes = await inspection.prepareChangeSet(
        ctx,
        {
          stackName: name,
          templateUrl,
          parameters: parameters.changeset,
          roleArn,
          templateDigest: digest(desired.template),
          requestId: env.DELIVERY_REQUEST_ID,
        },
        { api, delay: options.delay },
      );
      findings.push(
        ...inspection.resourceFindings(
          before.template,
          desired.template,
          changes.changes,
        ),
      );
      if (
        changes.status === "NO_CHANGE" &&
        digest(before.template) !== digest(desired.template)
      )
        findings.push({
          code: "AMBIGUOUS_EMPTY_CHANGE_SET",
          path: name,
          detail: "No-change response disagrees with effective templates",
        });
      plans.push({
        stackName: name,
        boundary: name.split("-").at(-1),
        template: desired.template,
        artifact,
        parameters: parameters.native,
        preservedParametersDigest: digest(
          (actual.Parameters || []).filter(
            (item) => !Object.hasOwn(values, item.ParameterKey),
          ),
        ),
        changeSet: changes,
      });
    }
    const manualInput = env.DELIVERY_MANUAL_COMPLETION
      ? JSON.parse(env.DELIVERY_MANUAL_COMPLETION)
      : {};
    const search = readEvidence(
        operational,
        store,
        manualInput.beforeObservation,
      ),
      prerequisites = searchPrerequisites(ctx, bundle, source, search);
    findings.push(...prerequisites.findings);
    const inventory = {
      candidateBundleDigest: digest(bundle),
      source,
      database,
      schema: snapshots,
      aws: live,
      dmsBaseline: {
        arn: task.ReplicationTaskArn,
        status: task.Status,
        settingsHash: require("./health.cjs").fingerprint(
          task.ReplicationTaskSettings,
        ),
        mappingsHash: require("./health.cjs").fingerprint(task.TableMappings),
      },
      baselineState: baseline,
      config: operational,
      plans,
      templateComparisons: Object.fromEntries(Object.entries(templates).map(([name, desired]) => [name, {
        beforeDigest: digest(live.stacks[name].template),
        afterDigest: digest(desired.template),
      }])),
      search,
      patches: prerequisites.patches,
      manualPrerequisites: prerequisites.manual,
      tests,
      hardFindings: findings,
      capturedRelations: capturedRelations(task),
      observedAt: new Date().toISOString(),
      operationalEvidence: reviewObservations.collect(ctx, {
        baseline, bundle, live, checks: manualInput.nativeChecks,
        repo: env.DELIVERY_REPO_DIR || ".",
      }, { api, store }),
    };
    const inventoryRef = store.putImmutableJson(
      ctx.bucket,
      `reports/evidence/${digest(inventory)}.json`,
      inventory,
    );
    const binding = {
      requestId: env.DELIVERY_REQUEST_ID,
      target: ctx.target,
      account: ctx.account,
      region: ctx.region,
      baseline: {
        revision: baseline.revision,
        bundleDigest: baseline.lastVerified.bundleDigest,
        sourceSha: baseline.lastVerified.sourceSha,
      },
      migrationLedgerDigest: database.ledger_fingerprint,
      candidateBundleDigest: digest(bundle),
      configDigest: digest(operational),
      templateDigest: digest(
        plans.map((item) => ({
          stack: item.stackName,
          template: item.artifact.sha256,
        })),
      ),
      changeSetDigest: digest(plans.map((item) => item.changeSet)),
      inventory: findings.map((item) => ({
        category: "HARD_POLICY",
        path: item.path,
        change: item.code,
      })),
      evidence: [
        {
          kind: "INVENTORY",
          uri: `s3://${inventoryRef.bucket}/${inventoryRef.key}?versionId=${encodeURIComponent(inventoryRef.versionId)}`,
          sha256: inventoryRef.sha256,
        },
      ],
      manualPrerequisites: prerequisites.manual,
    };
    const reviewed = await reviewer.review(
      ctx,
      inventory,
      binding,
      operational.reviewer,
      { api, fetch: options.fetch },
    );
    const modelRef = store.putImmutableJson(
      ctx.bucket,
      `reports/reviewer/${reviewed.report.reviewEvidenceDigest}.json`,
      reviewed.modelEvidence,
    );
    reviewed.report.evidence.push({
      kind: "MODEL_REVIEW",
      uri: `s3://${modelRef.bucket}/${modelRef.key}?versionId=${encodeURIComponent(modelRef.versionId)}`,
      sha256: modelRef.sha256,
    });
    validateReport(reviewed.report);
    const reportRef = store.putImmutableJson(
      ctx.bucket,
      `reports/${digest(reviewed.report)}.json`,
      reviewed.report,
    );
    fs.mkdirSync(".delivery", { recursive: true });
    fs.writeFileSync(
      ".delivery/report.json",
      JSON.stringify(reviewed.report, null, 2) + "\n",
    );
    const approval = {
      requestId: env.DELIVERY_REQUEST_ID,
      target: ctx.target,
      bundleDigest: digest(bundle),
      reportKey: reportRef.key,
      reportVersion: reportRef.versionId,
      reportDigest: digest(reviewed.report),
      prerequisiteDigest: reviewed.report.prerequisiteDigest,
      verdict: reviewed.report.verdict,
      account: ctx.account,
      region: ctx.region,
    };
    fs.writeFileSync(
      ".delivery/approval.json",
      JSON.stringify(approval, null, 2) + "\n",
    );
    for (const patch of prerequisites.patches)
      fs.writeFileSync(path.join(".delivery", patch.name), patch.content);
    fs.writeFileSync(".delivery/summary.md", reviewer.render(reviewed.report));
    if (env.GITHUB_STEP_SUMMARY)
      fs.appendFileSync(
        env.GITHUB_STEP_SUMMARY,
        reviewer.render(reviewed.report),
      );
    return { approval, report: reviewed.report, inventory };
  } finally {
    state.close?.();
  }
}
module.exports = {
  loadTargetConfig,
  context,
  output,
  discover,
  artifactParameters,
  effectiveParameters,
  candidateTemplates,
  capturedRelations,
  readTask,
  readEvidence,
  searchPrerequisites,
  prepare,
};
