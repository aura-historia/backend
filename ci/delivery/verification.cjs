"use strict";
const { digest } = require("./contracts.cjs");
const state = require("./admission.cjs");
const inspection = require("./inspection.cjs");
const preparation = require("./preparation.cjs");
const { verifyHealth } = require("./health.cjs");

function verifyArtifacts(ctx, bundle, plans, live, api) {
  const values = preparation.artifactParameters(bundle);
  for (const plan of plans) {
    const current = live.stacks[plan.stackName];
    if (!current || digest(current.template) !== digest(plan.template))
      throw new Error(
        "Effective deployed stack template differs from approved candidate",
      );
    const actual = api(ctx, "cloudformation", "describe-stacks", [
      "--stack-name",
      plan.stackName,
    ]).Stacks?.[0];
    if (!actual) throw new Error("Missing deployed parameter evidence");
    for (const [key, value] of Object.entries(plan.parameters))
      if (
        actual.Parameters?.find((item) => item.ParameterKey === key)
          ?.ParameterValue !== value
      )
        throw new Error(
          `Deployed ${key} differs from approved artifact/configuration`,
        );
    if (
      digest(
        (actual.Parameters || []).filter(
          (item) => !Object.hasOwn(values, item.ParameterKey),
        ),
      ) !== plan.preservedParametersDigest
    )
      throw new Error("Preserved effective parameters changed");
    const functions = new Map();
    for (const [logicalId, resource] of Object.entries(plan.template.Resources))
      if (resource.Type === "AWS::Lambda::Function") {
        const code = resource.Properties.Code;
        const key =
          typeof code.S3Key === "string" ? code.S3Key : values[code.S3Key?.Ref];
        if (!key) continue;
        const artifact = bundle.artifacts.find((item) => item.key === key);
        const physical = current.resources.find(
          (item) => item.logicalId === logicalId,
        )?.physicalId;
        if (!artifact || !physical)
          throw new Error("Unbound deployed Lambda artifact");
        const identity = api(ctx, "lambda", "get-function-configuration", [
          "--function-name",
          physical,
        ]);
        if (
          identity.CodeSha256 !==
          Buffer.from(artifact.sha256, "hex").toString("base64")
        )
          throw new Error(
            "Deployed Lambda code bytes differ from immutable bundle",
          );
        functions.set(logicalId, { physical, artifact });
      }
    for (const [logicalId, resource] of Object.entries(plan.template.Resources))
      if (resource.Type === "AWS::Lambda::Alias") {
        const name = resource.Properties.FunctionName,
          logical = name.Ref || name["Fn::GetAtt"]?.[0],
          expected = functions.get(logical);
        if (!expected)
          throw new Error(
            "Cannot resolve exact deployed alias artifact identity",
          );
        const alias = api(ctx, "lambda", "get-alias", [
          "--function-name",
          expected.physical,
          "--name",
          resource.Properties.Name,
        ]);
        if (
          Object.keys(alias.RoutingConfig?.AdditionalVersionWeights || {})
            .length
        )
          throw new Error("Alias canary has not settled");
        const code = api(ctx, "lambda", "get-function-configuration", [
          "--function-name",
          expected.physical,
          "--qualifier",
          alias.FunctionVersion,
        ]);
        if (
          code.CodeSha256 !==
          Buffer.from(expected.artifact.sha256, "hex").toString("base64")
        )
          throw new Error(
            `Alias ${logicalId} still serves different release bytes`,
          );
      }
  }
}
function physicalTarget(ctx, template, reference, resources) {
  const pseudo = {
    "AWS::Partition": "aws",
    "AWS::Region": ctx.region,
    "AWS::AccountId": ctx.account,
  };
  if (reference?.Ref && Object.hasOwn(pseudo, reference.Ref))
    return pseudo[reference.Ref];
  if (reference?.["Fn::Join"])
    return reference["Fn::Join"][1]
      .map((part) => physicalTarget(ctx, template, part, resources))
      .join(reference["Fn::Join"][0]);
  if (typeof reference === "string") return reference;
  const id = reference?.Ref || reference?.["Fn::GetAtt"]?.[0],
    resource = template.Resources[id],
    physical = resources.find((item) => item.logicalId === id)?.physicalId;
  if (!resource || !physical)
    throw new Error("Cannot resolve exact candidate consumer target version");
  if (resource.Type === "AWS::ECS::Cluster")
    return physical.startsWith("arn:")
      ? physical
      : `arn:aws:ecs:${ctx.region}:${ctx.account}:cluster/${physical}`;
  if (resource.Type === "AWS::Lambda::Function")
    return `arn:aws:lambda:${ctx.region}:${ctx.account}:function:${physical}`;
  if (
    [
      "AWS::Lambda::Version",
      "AWS::Lambda::Alias",
      "AWS::ECS::TaskDefinition",
    ].includes(resource.Type)
  )
    return physical;
  throw new Error(
    "Consumer target is outside candidate function/task ownership",
  );
}
function routineOwnership(plans, live) {
  const functions = [],
    tasks = [];
  for (const plan of plans)
    for (const [id, resource] of Object.entries(plan.template.Resources)) {
      if (resource.Type === "AWS::Lambda::Function")
        functions.push(
          live.stacks[plan.stackName].resources.find(
            (item) => item.logicalId === id,
          )?.physicalId,
        );
      if (resource.Type === "AWS::ECS::TaskDefinition")
        tasks.push(resource.Properties.Family);
    }
  return { functions, tasks };
}
function verifyConsumerArtifacts(ctx, bundle, plans, live, api) {
  const owned = routineOwnership(plans, live);
  for (const plan of plans) {
    const deployed = live.stacks[plan.stackName];
    for (const [id, resource] of Object.entries(plan.template.Resources)) {
      const physical = deployed.resources.find(
        (item) => item.logicalId === id,
      )?.physicalId;
      if (resource.Type === "AWS::Lambda::EventSourceMapping") {
        const mapping = live.activation.find((item) => item.id === physical);
        if (
          !mapping ||
          mapping.functionArn !==
            physicalTarget(
              ctx,
              plan.template,
              resource.Properties.FunctionName,
              deployed.resources,
            )
        )
          throw new Error(
            "Worker/router mapping does not target exact candidate immutable version",
          );
      } else if (resource.Type === "AWS::Scheduler::Schedule") {
        const schedule = live.activation.find(
          (item) =>
            item.group ===
              inspection.schedulerGroup(plan.template, { logicalId: id }) &&
            (item.id === physical || item.id.endsWith(`/${physical}`)),
        );
        if (
          !schedule ||
          (resource.Properties.Target.EcsParameters
            ? schedule.target.taskDefinitionArn
            : schedule.target.arn) !==
            physicalTarget(
              ctx,
              plan.template,
              resource.Properties.Target.EcsParameters?.TaskDefinitionArn ||
                resource.Properties.Target.Arn,
              deployed.resources,
            )
        )
          throw new Error(
            "Scheduler does not target exact candidate immutable version",
          );
      } else if (resource.Type === "AWS::Events::Rule") {
        const rule = live.activation.find(
          (item) =>
            item.bus ===
              (physical.includes("|") ? physical.split("|")[0] : "default") &&
            (item.id === physical ||
              item.id.endsWith(`/${physical.split("|").at(-1)}`)),
        );
        if (!rule) throw new Error("Missing deployed rule targets");
        for (const target of resource.Properties.Targets || [])
          if (target.EcsParameters) {
            const actual = rule.targets.find((item) => item.id === target.Id);
            if (
              actual?.taskDefinitionArn !==
              physicalTarget(
                ctx,
                plan.template,
                target.EcsParameters.TaskDefinitionArn,
                deployed.resources,
              )
            )
              throw new Error("Scheduled task revision differs from candidate");
          }
      } else if (resource.Type === "AWS::ECS::TaskDefinition") {
        const task = api(ctx, "ecs", "describe-task-definition", [
          "--task-definition",
          physical,
        ]).taskDefinition;
        if (
          !task ||
          task.taskDefinitionArn !== physical ||
          task.status !== "ACTIVE"
        )
          throw new Error("Candidate task definition identity missing");
        for (const image of bundle.images)
          if (
            !task.containerDefinitions.some((container) =>
              container.image.endsWith(`/${image.repository}@${image.digest}`),
            )
          )
            throw new Error(
              "Candidate scheduled image digest differs from immutable bundle",
            );
      }
    }
  }
  const targets = live.activation
    .flatMap((item) => [
      ...(item.functionArn ? [item.functionArn] : []),
      ...(item.target ? [item.target.arn] : []),
      ...(item.targets || []).map((target) => target.arn),
    ])
    .filter((arn) => arn.startsWith("arn:aws:lambda:"));
  for (const arn of new Set(targets)) {
    const functionName = arn.split(":")[6];
    if (!owned.functions.includes(functionName)) continue;
    const artifact = bundle.artifacts.find(
      (item) =>
        item.kind === "LAMBDA" && `${item.id}-${ctx.target}` === functionName,
    );
    if (!artifact)
      throw new Error("Consumer function is outside immutable bundle catalog");
    const deployed = api(ctx, "lambda", "get-function-configuration", [
      "--function-name",
      arn,
    ]);
    if (
      deployed.CodeSha256 !==
      Buffer.from(artifact.sha256, "hex").toString("base64")
    )
      throw new Error(
        "Consumer immutable version serves different artifact bytes",
      );
  }
}
async function verifyCurrentHealth(
  ctx,
  config,
  baselineDms,
  startedAt,
  options = {},
) {
  const api = options.api || state.aws;
  const live =
    options.live ||
    inspection.collectAws(ctx, config.stackNames, config.dmsTaskArn, api);
  const inventory = {
    ...healthInventory(ctx, live, api),
    http: {
      apiHost: new URL(config.apiUrl).host,
      mcpHost: new URL(config.mcpUrl).host,
    },
  };
  const router =
    options.routerBaseline ||
    live.activation.find((item) => item.id === inventory.routerMappingId);
  let health;
  for (let attempt = 0; attempt < (options.attempts || 6); attempt++) {
    health = await verifyHealth(
      {
        target: ctx.target,
        startedAt,
        inventory,
        baseline: {
          dmsTask: baselineDms,
          router: {
            uuid: router.id,
            state: router.state,
            eventSourceArn: router.source,
            functionArn: router.functionArn,
          },
        },
      },
      {
        api: async (service, operation, input) =>
          api(ctx, service, operation, [
            "--cli-input-json",
            JSON.stringify(input),
          ]),
        ...(options.fetch ? { fetch: options.fetch } : {}),
      },
    );
    if (
      health.status !== "UNVERIFIED" ||
      attempt === (options.attempts || 6) - 1
    )
      break;
    await (
      options.delay ||
      ((ms) => new Promise((resolve) => setTimeout(resolve, ms)))
    )(20000);
  }
  if (options.requirePass !== false && health.status !== "PASS")
    throw new Error(
      "Current CDC/WAL/worker/public health failed or remains unverified",
    );
  return health;
}
function healthInventory(ctx, live, api) {
  const resources = Object.values(live.stacks).flatMap(
    (stack) => stack.resources,
  );
  const single = (type) => {
    const found = resources.filter((item) => item.type === type);
    if (found.length !== 1)
      throw new Error(`Unambiguous ${type} health inventory required`);
    return found[0].physicalId;
  };
  const instanceArn = single("AWS::DMS::ReplicationInstance");
  const instances = api(ctx, "dms", "describe-replication-instances", [
    "--filters",
    `Name=replication-instance-arn,Values=${instanceArn}`,
  ]).ReplicationInstances;
  if (instances?.length !== 1)
    throw new Error("DMS health instance identity unavailable");
  const streamName = single("AWS::Kinesis::Stream"),
    router = live.activation.find(
      (item) =>
        item.source ===
        `arn:aws:kinesis:${ctx.region}:${ctx.account}:stream/${streamName}`,
    );
  if (!router) throw new Error("Router mapping health identity unavailable");
  const queues = [];
  for (const resource of resources.filter(
    (item) => item.type === "AWS::SQS::Queue",
  )) {
    const attrs = api(ctx, "sqs", "get-queue-attributes", [
      "--queue-url",
      resource.physicalId,
      "--attribute-names",
      "QueueArn",
      "RedrivePolicy",
    ]).Attributes;
    if (!attrs?.QueueArn) throw new Error("Queue identity unavailable");
    if (!attrs.RedrivePolicy) continue;
    const dead = JSON.parse(attrs.RedrivePolicy).deadLetterTargetArn;
    const name = attrs.QueueArn.split(":").at(-1),
      dlqUrl = api(ctx, "sqs", "get-queue-url", [
        "--queue-name",
        dead.split(":").at(-1),
      ]).QueueUrl;
    const mapping = live.activation.find(
      (item) => item.source === attrs.QueueArn,
    );
    queues.push({
      name,
      url: resource.physicalId,
      dlqUrl,
      ...(mapping
        ? { workerFunctionName: mapping.functionArn.split(":")[6] }
        : {}),
    });
  }
  return {
    dmsTaskArn: live.capture.arn,
    dmsInstanceId: instances[0].ReplicationInstanceIdentifier,
    databaseId: single("AWS::RDS::DBInstance"),
    streamName,
    routerMappingId: router.id,
    queues,
  };
}
async function smoke(config, fetchRequest = fetch) {
  const { probes, assertResponse } = require("./codedeploy-hook.cjs");
  const results = [];
  for (const [kind, url] of [
    ["api", config.apiUrl],
    ["mcp", config.mcpUrl],
  ]) {
    if (!/^https:\/\//.test(url || ""))
      throw new Error("Approved public API/MCP endpoint URLs required");
    for (const probe of probes(kind)) {
      const response = await fetchRequest(`${url}${probe.path}`, {
        method: probe.body ? "POST" : "GET",
        redirect: "error",
        signal: AbortSignal.timeout(20000),
        headers: {
          "cache-control": "no-cache",
          accept: "application/json,text/event-stream",
          ...(probe.body
            ? {
                "content-type": "application/json",
                "mcp-protocol-version": "2026-07-28",
                "mcp-method": "tools/call",
                "mcp-name": probe.body.params.name,
              }
            : {}),
        },
        ...(probe.body ? { body: JSON.stringify(probe.body) } : {}),
      });
      assertResponse(probe, {
        statusCode: response.status,
        body: await response.text(),
      });
      results.push({ kind, path: probe.path, status: response.status });
    }
  }
  return results;
}
module.exports = {
  routineOwnership,
  verifyArtifacts,
  physicalTarget,
  verifyConsumerArtifacts,
  verifyCurrentHealth,
  healthInventory,
  smoke,
};
