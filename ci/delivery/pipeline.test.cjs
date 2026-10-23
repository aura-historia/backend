"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const contracts = require("./contracts.cjs");
const fixture = require("./fixtures.cjs");
const preparation = require("./preparation.cjs");
const execution = require("./execution.cjs");
const inspection = require("./inspection.cjs");

const {
  versionTemplate,
  mappingChange,
  pipelineFixture,
} = require("./fixtures/pipeline.cjs");
test("worker immutable version advances on the same function while redirect and activation changes hold", () => {
  const before = versionTemplate("VersionOld"),
    after = versionTemplate("VersionNew");
  assert.deepEqual(
    inspection.resourceFindings(before, after, [mappingChange]),
    [],
  );
  const redirect = versionTemplate("VersionNew", "unrelated-worker-prod");
  assert.ok(
    inspection
      .resourceFindings(before, redirect, [mappingChange])
      .some((item) => item.code === "CONSUMER_TOPOLOGY_OR_ACTIVATION_CHANGED"),
  );
  const disabled = structuredClone(after);
  disabled.Resources.Mapping.Properties.Enabled = false;
  assert.ok(
    inspection.resourceFindings(before, disabled, [mappingChange]).length,
  );
  const scheduled = (version) => ({
    Resources: {
      Group: {
        Type: "AWS::Scheduler::ScheduleGroup",
        Properties: { Name: "matcher-prod" },
      },
      [version]: {
        Type: "AWS::ECS::TaskDefinition",
        Properties: { Family: "matcher-prod" },
      },
      Schedule: {
        Type: "AWS::Scheduler::Schedule",
        Properties: {
          GroupName: { Ref: "Group" },
          State: { "Fn::If": ["Enabled", "ENABLED", "DISABLED"] },
          Target: {
            Arn: { "Fn::GetAtt": ["Cluster", "Arn"] },
            RoleArn: "arn:role",
            EcsParameters: {
              TaskDefinitionArn: { Ref: version },
              TaskCount: 1,
            },
            RetryPolicy: { MaximumRetryAttempts: 2 },
          },
        },
      },
    },
  });
  const scheduleChange = {
    ResourceChange: {
      LogicalResourceId: "Schedule",
      ResourceType: "AWS::Scheduler::Schedule",
      Action: "Modify",
      Replacement: "False",
      Details: [
        {
          Target: {
            Attribute: "Properties",
            Name: "Target",
            RequiresRecreation: "Never",
          },
        },
      ],
    },
  };
  assert.deepEqual(
    inspection.resourceFindings(scheduled("TaskOld"), scheduled("TaskNew"), [
      scheduleChange,
    ]),
    [],
  );
  assert.equal(
    inspection.schedulerGroup(scheduled("TaskOld"), { logicalId: "Schedule" }),
    "matcher-prod",
  );
  assert.equal(
    execution.physicalTarget(
      { region: "eu-central-1", account: "123456789012" },
      { Resources: {} },
      {
        "Fn::Join": [
          "",
          [
            "arn:",
            { Ref: "AWS::Partition" },
            ":lambda:",
            { Ref: "AWS::Region" },
            ":",
            { Ref: "AWS::AccountId" },
            ":function:fxrate-lambda-prod",
          ],
        ],
      },
      [],
    ),
    "arn:aws:lambda:eu-central-1:123456789012:function:fxrate-lambda-prod",
  );
  const rule = {
    Resources: {
      Rule: {
        Properties: {
          EventBusName:
            "{{resolve:ssm:/eventbridge/prod/stripe-event-bus-name}}",
        },
      },
    },
  };
  assert.equal(
    inspection.eventBus(
      { target: "prod" },
      rule,
      { logicalId: "Rule" },
      (_ctx, _service, _op, args) => {
        assert.equal(args[1], "/eventbridge/prod/stripe-event-bus-name");
        return { Parameter: { Type: "String", Value: "partner-bus" } };
      },
    ),
    "partner-bus",
  );
  const fxReference = {
    "Fn::Join": [
      "",
      [
        "arn:",
        { Ref: "AWS::Partition" },
        ":lambda:",
        { Ref: "AWS::Region" },
        ":",
        { Ref: "AWS::AccountId" },
        ":function:fxrate-lambda-prod",
      ],
    ],
  };
  const plan = {
    stackName: "application-prod-compute",
    template: {
      Resources: {
        FxSchedule: {
          Type: "AWS::Scheduler::Schedule",
          Properties: { Target: { Arn: fxReference } },
        },
        PartnerRule: { Type: "AWS::Events::Rule", Properties: { Targets: [] } },
      },
    },
  };
  execution.verifyConsumerArtifacts(
    { target: "prod", region: "eu-central-1", account: "123456789012" },
    fixture.bundle(),
    [plan],
    {
      stacks: {
        "application-prod-compute": {
          resources: [
            { logicalId: "FxSchedule", physicalId: "fx" },
            {
              logicalId: "PartnerRule",
              physicalId: "partner-bus|partner-rule",
            },
          ],
        },
      },
      activation: [
        {
          id: "arn:scheduler:default/fx",
          group: "default",
          target: {
            arn: "arn:aws:lambda:eu-central-1:123456789012:function:fxrate-lambda-prod",
          },
        },
        {
          id: "arn:events:rule/partner-bus/partner-rule",
          bus: "partner-bus",
          targets: [],
        },
      ],
    },
    () => {
      throw new Error(
        "Protected FX must retain baseline bytes instead of consuming candidate artifact",
      );
    },
  );
});

test("prepare through native admission, candidate migration and verified worker rollout shares exact evidence", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "delivery-flow-")),
    previous = process.cwd();
  const { digest } = contracts;
  try {
    process.chdir(directory);
    const scenario = pipelineFixture(directory);
    const {
      store,
      api,
      stateStore,
      env,
      schema,
      bundleDigest,
      tests,
      candidate,
    } = scenario;
    const prepared = await preparation.prepare(env, {
      store,
      api,
      stateStore,
      snapshots: async () => ({
        before: schema,
        after: schema,
        image: "isolated-fixture",
      }),
    });
    assert.equal(
      prepared.report.verdict,
      "ROUTINE",
      JSON.stringify(prepared.report.findings),
    );
    assert.ok(
      prepared.report.evidence.some((item) => item.kind === "MODEL_REVIEW"),
    );
    const request = {
      ...fixture.request(),
      id: env.DELIVERY_REQUEST_ID,
      sourceSha: candidate,
      workflowRevision: tests.workflowRevision,
      testEvidenceDigest: digest(tests),
      bundleDigest,
      authorization: {
        scope: "DEPLOY",
        environment: "aws-prod",
        approvedAt: new Date().toISOString(),
      },
    };
    const key = `requests/prod/${request.id}.json`,
      requestRef = store.putImmutableJson(env.DELIVERY_BUCKET, key, request);
    store.putImmutableJson(
      env.DELIVERY_BUCKET,
      `${key.slice(0, -5)}/tests.json`,
      tests,
    );
    store.putImmutableJson(
      env.DELIVERY_BUCKET,
      `requests/prod/${request.id}-manual.json`,
      {
        target: "prod",
        reportDigest: digest(prepared.report),
        prerequisiteDigest: prepared.report.prerequisiteDigest,
        confirmed: true,
        observations: [],
      },
    );
    const nativeEnv = {
      ...env,
      DELIVERY_REQUEST_KEY: key,
      DELIVERY_REQUEST_VERSION: requestRef.versionId,
      DELIVERY_REQUEST_DIGEST: digest(request),
      DELIVERY_REPORT_KEY: prepared.approval.reportKey,
      DELIVERY_REPORT_VERSION: prepared.approval.reportVersion,
      DELIVERY_REPORT_DIGEST: prepared.approval.reportDigest,
      DELIVERY_EXECUTION_ID: "native-execution",
    };
    scenario.setDmsStatus("stopped");
    await assert.rejects(
      execution.admit(nativeEnv, { store, api, stateStore }),
      /capture lifecycle/,
    );
    assert.equal(scenario.environment.state, "READY");
    scenario.setDmsStatus("running");
    await execution.admit(nativeEnv, { store, api, stateStore });
    assert.equal(scenario.environment.state, "IN_PROGRESS");
    assert.equal(
      JSON.parse(fs.readFileSync("prepared/action-variables.json"))
        .ComputeActionMode,
      "CHANGE_SET_EXECUTE",
    );
    scenario.setPhase("migrate");
    await execution.migrate(nativeEnv, { store, api, stateStore });
    assert.equal(scenario.environment.state, "IN_PROGRESS");
    scenario.setPhase("complete");
    const fetch = async (url, options) => ({
      status: url.includes("/ready")
        ? 204
        : url.includes("/v1/me/")
          ? 401
          : 200,
      text: async () =>
        options?.body
          ? JSON.stringify({ jsonrpc: "2.0", id: 1, result: { content: [] } })
          : "{}",
    });
    await execution.verify(nativeEnv, { store, api, stateStore, fetch });
    assert.equal(scenario.environment.state, "READY");
    assert.equal(scenario.environment.lastVerified.bundleDigest, bundleDigest);
    assert.equal(scenario.environment.lastVerified.sourceSha, candidate);
  } finally {
    process.chdir(previous);
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
