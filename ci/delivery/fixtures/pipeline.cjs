"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { digest, bytesDigest, canonicalize } = require("../contracts.cjs");
const fixture = require("../fixtures.cjs");
const { REQUIRED_JOBS } = require("../gate.cjs");
const preparation = require("../preparation.cjs");

function versionTemplate(versionId, target = "cdc-router-lambda-prod") {
  return {
    Parameters: {
      CommitSHA: { Type: "String" },
      CdcRouterLambdaArtifactKey: { Type: "String" },
      CdcRouterLambdaArtifactVersion: { Type: "String" },
      CdcRouterLambdaArtifactSha256: { Type: "String" },
    },
    Resources: {
      Worker: {
        Type: "AWS::Lambda::Function",
        Properties: {
          FunctionName: target,
          Code: {
            S3Key: { Ref: "CdcRouterLambdaArtifactKey" },
            S3ObjectVersion: { Ref: "CdcRouterLambdaArtifactVersion" },
          },
        },
      },
      [versionId]: {
        Type: "AWS::Lambda::Version",
        Properties: {
          FunctionName: { Ref: "Worker" },
          CodeSha256: { Ref: "CdcRouterLambdaArtifactSha256" },
        },
      },
      Mapping: {
        Type: "AWS::Lambda::EventSourceMapping",
        Properties: {
          FunctionName: { Ref: versionId },
          EventSourceArn:
            "arn:aws:kinesis:eu-central-1:123456789012:stream/cdc",
          Enabled: true,
          BatchSize: 100,
          StartingPosition: "TRIM_HORIZON",
        },
      },
    },
  };
}
const mappingChange = {
  ResourceChange: {
    LogicalResourceId: "Mapping",
    ResourceType: "AWS::Lambda::EventSourceMapping",
    Action: "Modify",
    Replacement: "False",
    Details: [
      {
        Target: {
          Attribute: "Properties",
          Name: "FunctionName",
          RequiresRecreation: "Never",
        },
      },
    ],
  },
};
function pipelineFixture(directory) {
  for (const folder of ["docs/events", "migrations", "src/worker", ".delivery"])
    fs.mkdirSync(folder, { recursive: true });
  const sql = "CREATE TABLE captured(id bigint PRIMARY KEY);";
  fs.writeFileSync("migrations/1_initial.sql", sql);
  fs.writeFileSync("docs/events/flow.md", "Versioned CDC contract");
  fs.writeFileSync("docs/durable-worker-runbook.md", "Preserve retained jobs");
  fs.writeFileSync("src/worker/lib.rs", "version 1");
  const git = (args) =>
    execFileSync(
      "git",
      [
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        ...args,
      ],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    ).trim();
  git(["init", "--quiet"]);
  git(["add", "."]);
  git(["commit", "--quiet", "-m", "baseline"]);
  const baselineSha = git(["rev-parse", "HEAD"]);
  fs.writeFileSync("src/worker/lib.rs", "version 2; same queued contract");
  git(["add", "."]);
  git(["commit", "--quiet", "-m", "compatible worker"]);
  const candidate = git(["rev-parse", "HEAD"]);
  const objects = new Map();
  let version = 0;
  const store = {
    putImmutableJson(bucket, key, value) {
      const body = Buffer.from(canonicalize(value));
      if (objects.has(key))
        assert.equal(bytesDigest(objects.get(key).body), bytesDigest(body));
      else objects.set(key, { body, versionId: `v${++version}` });
      return {
        bucket,
        key,
        versionId: objects.get(key).versionId,
        sha256: bytesDigest(body),
        size: body.length,
      };
    },
    readObject(_bucket, key, pinned) {
      const object = objects.get(key);
      assert.ok(object, `missing ${key}`);
      if (pinned) assert.equal(pinned, object.versionId);
      return object;
    },
    readJson(bucket, key, pinned) {
      return JSON.parse(this.readObject(bucket, key, pinned).body);
    },
    aws(_service, operation, args) {
      assert.equal(operation, "list-objects-v2");
      return {
        Contents: [...objects.keys()]
          .filter((key) => key.startsWith(args[args.indexOf("--prefix") + 1]))
          .map((Key) => ({ Key })),
      };
    },
  };
  const asset = (id, kind, body) => {
    body = Buffer.from(body);
    const sha256 = bytesDigest(body),
      key = `objects/sha256/${sha256}`;
    objects.set(key, { body, versionId: `v${++version}` });
    return {
      id,
      kind,
      bucket: "release-bucket",
      key,
      versionId: objects.get(key).versionId,
      sha256,
      size: body.length,
    };
  };
  const migration = asset(
      "database-migration-lambda",
      "LAMBDA",
      "migration-zip",
    ),
    worker = asset("cdc-router-lambda", "LAMBDA", "candidate-worker-zip"),
    assembly = asset("assembly-prod", "ASSEMBLY", "assembly");
  const prefix = "application-prod-",
    names = [
      "network",
      "data",
      "identity",
      "initialize",
      "migrate",
      "compute",
      "api",
      "observability",
    ].map((suffix) => prefix + suffix);
  const basic = (resources) => ({ Resources: resources });
  const templates = {
    network: basic({ VPC: { Type: "AWS::EC2::VPC", Properties: {} } }),
    data: basic({
      DB: { Type: "AWS::RDS::DBInstance", Properties: {} },
      Task: { Type: "AWS::DMS::ReplicationTask", Properties: {} },
      Instance: { Type: "AWS::DMS::ReplicationInstance", Properties: {} },
      Stream: { Type: "AWS::Kinesis::Stream", Properties: {} },
      Queue: { Type: "AWS::SQS::Queue", Properties: {} },
      DLQ: { Type: "AWS::SQS::Queue", Properties: {} },
    }),
    identity: basic({
      Identity: { Type: "AWS::Cognito::UserPool", Properties: {} },
    }),
    initialize: basic({
      Inspector: {
        Type: "AWS::Lambda::Function",
        Properties: {
          FunctionName: "database-inspection-lambda-prod",
          Code: { S3Key: "protected-inspector" },
        },
      },
    }),
    migrate: {
      Parameters: {
        CommitSHA: { Type: "String" },
        DatabaseMigrationLambdaArtifactKey: { Type: "String" },
        DatabaseMigrationLambdaArtifactVersion: { Type: "String" },
        DatabaseMigrationLambdaArtifactSha256: { Type: "String" },
      },
      Resources: {
        Migrator: {
          Type: "AWS::Lambda::Function",
          Properties: {
            FunctionName: "database-migration-lambda-prod",
            Code: {
              S3Key: { Ref: "DatabaseMigrationLambdaArtifactKey" },
              S3ObjectVersion: {
                Ref: "DatabaseMigrationLambdaArtifactVersion",
              },
            },
          },
        },
      },
    },
    compute: versionTemplate("VersionNew"),
    api: basic({ Api: { Type: "AWS::ApiGatewayV2::Api", Properties: {} } }),
    observability: basic({
      Alarm: { Type: "AWS::CloudWatch::Alarm", Properties: {} },
    }),
  };
  const beforeTemplates = {
    ...templates,
    compute: versionTemplate("VersionOld"),
  };
  const templateAssets = Object.entries(templates).map(([suffix, value]) =>
    asset(
      `template-prod-${prefix}${suffix}.template.json`,
      "ASSET",
      JSON.stringify(value),
    ),
  );
  const tests = {
    ...fixture.tests(),
    sourceSha: candidate,
    workflowRevision: "d".repeat(40),
    requiredJobs: [...REQUIRED_JOBS],
    jobResults: Object.fromEntries(
      REQUIRED_JOBS.map((job) => [job, "success"]),
    ),
  };
  const bundle = {
    ...fixture.bundle(),
    sourceSha: candidate,
    tests,
    catalogs: { lambdas: [migration.id, worker.id], containers: [] },
    artifacts: [migration, worker, assembly, ...templateAssets],
    migrations: [
      {
        version: "1",
        checksum: createHash("sha384").update(sql).digest("hex"),
      },
    ],
    targets: {
      prod: {
        account: "123456789012",
        region: "eu-central-1",
        assemblyDigest: assembly.sha256,
        templateDigests: Object.fromEntries(
          templateAssets.map((item) => [item.id, item.sha256]),
        ),
      },
    },
  };
  bundle.build.id = digest({
    sourceSha: candidate,
    configDigest: bundle.build.configDigest,
  });
  const bundleDigest = digest(bundle);
  store.putImmutableJson(
    "release-bucket",
    `bundles/${bundleDigest}.json`,
    bundle,
  );
  fs.writeFileSync(".delivery/tests.json", JSON.stringify(tests));
  const schema = [
      {
        relation: "public.captured",
        columns: [{ name: "id", type: "bigint", nullable: false }],
        kind: "r",
        replica_identity: "d",
      },
    ],
    ledger = [
      { checksum: bundle.migrations[0].checksum, success: true, version: 1 },
    ];
  const beforeAt = new Date(Date.now() - 60000).toISOString();
  let deployedAt = beforeAt,
    phase = "before";
  let environment = {
    version: 1,
    target: "prod",
    revision: 0,
    state: "READY",
    lastVerified: {
      bundleDigest: fixture.hash,
      sourceSha: baselineSha,
      verifiedAt: beforeAt,
    },
    observedSchema: {
      sha256: digest(schema),
      observedAt: beforeAt,
      migrations: bundle.migrations,
    },
    active: null,
    holdReason: null,
  };
  const stateStore = {
    read: () => ({
      state: structuredClone(environment),
      etag: String(environment.revision),
    }),
    write: (next, etag) => {
      assert.equal(etag, String(environment.revision));
      environment = next;
    },
  };
  const fnArn = (name) =>
    `arn:aws:lambda:eu-central-1:123456789012:function:${name}`;
  const physical = {
    VPC: "vpc-1",
    DB: "database-prod",
    Task: "arn:task",
    Instance: "arn:instance",
    Stream: "cdc",
    Queue: "https://sqs.eu-central-1.amazonaws.com/123456789012/work",
    DLQ: "https://sqs.eu-central-1.amazonaws.com/123456789012/dead",
    Identity: "pool",
    Inspector: "database-inspection-lambda-prod",
    Migrator: "database-migration-lambda-prod",
    Worker: "cdc-router-lambda-prod",
    VersionOld: fnArn("cdc-router-lambda-prod") + ":1",
    VersionNew: fnArn("cdc-router-lambda-prod") + ":2",
    Mapping: "router-id",
    Api: "api",
    Alarm: "alarm",
  };
  const values = preparation.artifactParameters(bundle),
    oldValues = { ...values, CommitSHA: baselineSha };
  for (const key of Object.keys(oldValues))
    if (key.endsWith("ArtifactKey"))
      oldValues[key] = "objects/sha256/" + fixture.hash;
    else if (key.endsWith("ArtifactVersion")) oldValues[key] = "old-version";
    else if (key.endsWith("ArtifactSha256")) oldValues[key] = fixture.hash;
  const task = {
    ReplicationTaskArn: "arn:task",
    MigrationType: "cdc",
    SourceEndpointArn: "arn:source",
    TargetEndpointArn: "arn:target",
    ReplicationInstanceArn: "arn:instance",
    Status: "running",
    TableMappings: JSON.stringify({
      rules: [
        {
          "rule-type": "selection",
          "rule-action": "include",
          "object-locator": {
            "schema-name": "public",
            "table-name": "captured",
          },
        },
      ],
    }),
    ReplicationTaskSettings: "{}",
    ReplicationTaskStats: { TablesErrored: 0 },
  };
  const changesets = new Map(),
    changeId = (name) => `arn:changeset:${name}`;
  const stackFor = (name) => {
    const suffix = name.slice(prefix.length),
      updated =
        (suffix === "migrate" && phase !== "before") ||
        (suffix === "compute" && phase === "complete");
    const template = updated ? templates[suffix] : beforeTemplates[suffix];
    return {
      StackId: `arn:stack:${name}`,
      StackStatus: "UPDATE_COMPLETE",
      CreationTime: beforeAt,
      LastUpdatedTime: updated ? deployedAt : beforeAt,
      Parameters: Object.keys(template.Parameters || {}).map(
        (ParameterKey) => ({
          ParameterKey,
          ParameterValue: (updated ? values : oldValues)[ParameterKey],
        }),
      ),
      Outputs: [
        ...(suffix === "initialize"
          ? [
              {
                OutputKey: "DatabaseInspectionVersionArn",
                OutputValue: fnArn("database-inspection-lambda-prod") + ":1",
              },
            ]
          : []),
        ...(suffix === "migrate"
          ? [
              {
                OutputKey: "DatabaseMigrationVersionArn",
                OutputValue: fnArn("database-migration-lambda-prod") + ":2",
              },
            ]
          : []),
      ],
    };
  };
  const api = (_ctx, service, operation, args = []) => {
    const arg = (name) => args[args.indexOf(name) + 1],
      input = args.includes("--cli-input-json")
        ? JSON.parse(
            arg("--cli-input-json").startsWith("file://")
              ? fs.readFileSync(arg("--cli-input-json").slice(7))
              : arg("--cli-input-json"),
          )
        : {};
    if (operation === "get-caller-identity") return { Account: "123456789012" };
    if (operation === "describe-stacks")
      return { Stacks: [stackFor(arg("--stack-name"))] };
    if (operation === "list-stack-resources") {
      const name = arg("--stack-name").replace("arn:stack:", ""),
        suffix = name.slice(prefix.length),
        template =
          suffix === "compute" && phase !== "complete"
            ? beforeTemplates[suffix]
            : templates[suffix];
      return {
        StackResourceSummaries: Object.entries(template.Resources).map(
          ([LogicalResourceId, value]) => ({
            LogicalResourceId,
            ResourceType: value.Type,
            PhysicalResourceId: physical[LogicalResourceId],
          }),
        ),
      };
    }
    if (operation === "get-template") {
      const name = arg("--stack-name").replace("arn:stack:", ""),
        suffix = name.slice(prefix.length);
      return {
        TemplateBody: args.includes("--change-set-name")
          ? templates[suffix]
          : suffix === "compute" && phase !== "complete"
            ? beforeTemplates[suffix]
            : templates[suffix],
      };
    }
    if (operation === "get-function-configuration") {
      const name =
        arg("--function-name").split(":")[6] || arg("--function-name");
      const sha =
        name === "database-inspection-lambda-prod"
          ? fixture.hash
          : name === "database-migration-lambda-prod" && phase !== "before"
            ? migration.sha256
            : name === "cdc-router-lambda-prod" && phase === "complete"
              ? worker.sha256
              : fixture.hash;
      return {
        FunctionArn: fnArn(name),
        CodeSha256: Buffer.from(sha, "hex").toString("base64"),
        RevisionId:
          name !== "database-inspection-lambda-prod" && phase === "complete"
            ? "candidate"
            : "baseline",
        Version: arg("--function-name").endsWith(":2") ? "2" : "$LATEST",
        Role: "arn:role",
        Runtime: "provided.al2023",
        Architectures: ["x86_64"],
      };
    }
    if (operation === "get-event-source-mapping")
      return {
        UUID: "router-id",
        EventSourceArn: "arn:aws:kinesis:eu-central-1:123456789012:stream/cdc",
        FunctionArn:
          fnArn("cdc-router-lambda-prod") +
          (phase === "complete" ? ":2" : ":1"),
        State: "Enabled",
        StartingPosition: "TRIM_HORIZON",
      };
    if (operation === "describe-replication-tasks")
      return { ReplicationTasks: [task] };
    if (operation === "describe-endpoints") {
      const arn = arg("--filters").split("Values=")[1];
      return {
        Endpoints: [
          {
            EndpointArn: arn,
            EngineName: arn === "arn:source" ? "postgres" : "kinesis",
            Status: "active",
            DatabaseName: "postgres",
            PostgreSQLSettings: {
              SlotName: "slot",
              PluginName: "test_decoding",
            },
            KinesisSettings: {
              StreamArn: "arn:aws:kinesis:eu-central-1:123456789012:stream/cdc",
            },
          },
        ],
      };
    }
    if (operation === "describe-replication-instances")
      return {
        ReplicationInstances: [{ ReplicationInstanceIdentifier: "dms-prod" }],
      };
    if (operation === "describe-table-statistics")
      return { TableStatistics: [{ TableState: "Table completed" }] };
    if (operation === "create-change-set") {
      const name = arg("--stack-name"),
        suffix = name.slice(prefix.length),
        changes = ["migrate", "compute"].includes(suffix)
          ? [
              {
                ResourceChange: {
                  LogicalResourceId:
                    suffix === "migrate" ? "Migrator" : "Worker",
                  ResourceType: "AWS::Lambda::Function",
                  Action: "Modify",
                  Replacement: "False",
                },
              },
              ...(suffix === "compute" ? [mappingChange] : []),
            ]
          : [];
      const value = {
        ChangeSetId: changeId(name),
        StackId: `arn:stack:${name}`,
        Status: changes.length ? "CREATE_COMPLETE" : "FAILED",
        ExecutionStatus: changes.length ? "AVAILABLE" : "UNAVAILABLE",
        StatusReason: "The submitted information didn't contain changes.",
        Parameters: JSON.parse(arg("--parameters")),
        Changes: changes,
      };
      changesets.set(value.ChangeSetId, value);
      return { Id: value.ChangeSetId, StackId: value.StackId };
    }
    if (operation === "describe-change-set") {
      assert.ok(args.includes("--include-property-values"));
      return changesets.get(arg("--change-set-name"));
    }
    if (operation === "invoke") {
      const arn = arg("--function-name"),
        filename = args.at(-1);
      if (arn.includes("database-inspection"))
        fs.writeFileSync(
          filename,
          JSON.stringify({
            inspector_version: "database-inspector-v1",
            target: "prod",
            ledger,
            ledger_fingerprint: digest(ledger),
            schema,
          }),
        );
      else {
        const request = JSON.parse(
          fs.readFileSync(arg("--payload").slice("fileb://".length)),
        );
        assert.deepEqual(request.pending, []);
        fs.writeFileSync(
          filename,
          JSON.stringify({
            status: "NO_OP",
            request_id: request.request_id,
            bundle_digest: bundleDigest,
            target: "prod",
            ledger,
            ledger_fingerprint: digest(ledger),
          }),
        );
      }
      return { StatusCode: 200, ExecutedVersion: arn.split(":").at(-1) };
    }
    if (operation === "get-queue-attributes") {
      const url = arg("--queue-url") || input.QueueUrl;
      if ((arg("--attribute-names") || "").includes("QueueArn"))
        return {
          Attributes: {
            QueueArn:
              "arn:aws:sqs:eu-central-1:123456789012:" + url.split("/").at(-1),
            ...(url.endsWith("/work")
              ? {
                  RedrivePolicy: JSON.stringify({
                    deadLetterTargetArn:
                      "arn:aws:sqs:eu-central-1:123456789012:dead",
                  }),
                }
              : {}),
          },
        };
      return {
        Attributes: {
          ApproximateNumberOfMessages: "0",
          ApproximateNumberOfMessagesNotVisible: "0",
        },
      };
    }
    if (operation === "get-queue-url")
      return {
        QueueUrl: "https://sqs.eu-central-1.amazonaws.com/123456789012/dead",
      };
    if (operation === "get-metric-statistics") {
      const name = input.MetricName,
        value =
          name === "FreeStorageSpace"
            ? 10 * 1024 ** 3
            : name === "GetRecords.Success" || name === "CompletedRecords"
              ? 1
              : 0;
      return {
        Datapoints: [
          {
            Timestamp: deployedAt,
            Maximum: value,
            Minimum: value,
            Sum: value,
          },
        ],
      };
    }
    if (service === "ssm" && operation === "get-parameter")
      return { Parameter: { Name: "/anthropic/prod/api-key", Value: "test-key" } };
    throw new Error(`Unexpected API ${service}/${operation}`);
  };
  const env = {
    DELIVERY_BUCKET: "release-bucket",
    DELIVERY_TARGET: "prod",
    DELIVERY_BUNDLE_DIGEST: bundleDigest,
    DELIVERY_REQUEST_ID: "e".repeat(64),
    DELIVERY_TESTS_FILE: ".delivery/tests.json",
    DELIVERY_REPO_DIR: directory,
    AWS_ACCOUNT_ID: "123456789012",
    DELIVERY_REVIEW_MODEL_ID: "claude-opus-5-5",
    WORKFLOW_REVISION: tests.workflowRevision,
    INVOCATION_REVISION: tests.invocationRevision,
    GITHUB_RUN_ID: tests.runId,
    GITHUB_RUN_ATTEMPT: String(tests.runAttempt),
  };
  return {
    store,
    api,
    fetch: async () => new Response(JSON.stringify({
      type: "message", role: "assistant", model: "claude-opus-5-5", stop_reason: "end_turn",
      content: [{ type: "text", text: JSON.stringify({
        verdict: "ROUTINE", reasons: [{ code: "COMPATIBLE", path: "src/worker/lib.rs", detail: "Retained contract unchanged" }],
        oldNewCompatible: true, queuedMessagesCompatible: true, sqlCompatible: true, searchCompatible: true, uncertainties: [],
      }) }],
      usage: { input_tokens: 1, output_tokens: 1 },
    })),
    stateStore,
    env,
    schema,
    bundleDigest,
    tests,
    candidate,
    get environment() {
      return environment;
    },
    setDmsStatus(value) {
      task.Status = value;
    },
    setPhase(value) {
      phase = value;
      deployedAt = new Date().toISOString();
    },
  };
}
module.exports = { versionTemplate, mappingChange, pipelineFixture };
