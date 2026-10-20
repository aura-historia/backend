'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { aws, hash, command, sleep } = require('./io.cjs');
const { runtimeActivation } = require('./activation.cjs');
const HEALTHY = ['CREATE_COMPLETE', 'UPDATE_COMPLETE', 'IMPORT_COMPLETE'];
const names = (ctx) => [
  'network',
  'data',
  'initialize',
  'compute',
  'api',
  ...(ctx.stage === 'prod' ? ['observability'] : []),
];
const stackName = (ctx, name) => `application-${ctx.stage}-${name}`;
function parameters(stack) {
  if (!Array.isArray(stack.Parameters)) throw new Error('Stack parameter inventory missing.');
  const result = {};
  for (const p of stack.Parameters) {
    if (Object.hasOwn(result, p.ParameterKey)) throw new Error('Duplicate stack parameter.');
    result[p.ParameterKey] = p.ParameterValue;
  }
  return result;
}
function template(ctx, name, api) {
  const body = api(ctx, 'cloudformation', 'get-template', [
    '--stack-name',
    stackName(ctx, name),
    '--template-stage',
    'Original',
  ]).TemplateBody;
  const value = typeof body === 'string' ? JSON.parse(body) : body;
  if (!value?.Resources) throw new Error(`Missing template for ${name}.`);
  return value;
}
function normalized(value) {
  if (Array.isArray(value)) return value.map(normalized);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.entries(value)
      .filter(([key]) => key !== 'Metadata')
      .map(([key, child]) => [key, normalized(child)]),
  );
}
function inspect(ctx, api = aws) {
  const stacks = {};
  for (const name of names(ctx)) {
    const response = api(ctx, 'cloudformation', 'describe-stacks', ['--stack-name', stackName(ctx, name)]);
    const stack = response.Stacks?.length === 1 ? response.Stacks[0] : null;
    if (
      !stack?.StackId?.startsWith(
        `arn:aws:cloudformation:${ctx.region}:${ctx.account}:stack/${stackName(ctx, name)}/`,
      ) ||
      !HEALTHY.includes(stack.StackStatus)
    )
      throw new Error(
        `${name} is missing, unhealthy or updating. Inspect CloudFormation; first-time activation and stack repair use the exceptional procedure.`,
      );
    const resources = api(ctx, 'cloudformation', 'list-stack-resources', [
      '--stack-name',
      stack.StackId,
    ]).StackResourceSummaries;
    if (!resources?.length || resources.some((r) => !HEALTHY.includes(r.ResourceStatus)))
      throw new Error(`${name} has incomplete resources; inspect CloudFormation before continuing.`);
    stacks[name] = { stack, resources, template: template(ctx, name, api) };
  }
  return stacks;
}
function snapshot(stacks) {
  return Object.fromEntries(
    Object.entries(stacks).map(([name, { stack, resources, template: body }]) => [
      name,
      {
        id: stack.StackId,
        updatedAt: stack.LastUpdatedTime || stack.CreationTime,
        parameters: hash(stack.Parameters ? parameters(stack) : {}),
        template: hash(normalized(body)),
        resources: hash(
          resources.map((r) => [r.LogicalResourceId, r.PhysicalResourceId, r.ResourceType]).sort(),
        ),
      },
    ]),
  );
}
function activation(stacks) {
  const params = parameters(stacks.compute.stack);
  for (const name of ['CdcRouterEnabled', 'PeriodicMatcherEnabled'])
    if (!['true', 'false'].includes(params[name]))
      throw new Error(`Missing/invalid ${name}; establish explicit activation state before adoption.`);
  return Object.fromEntries(Object.entries(params).filter(([name]) => name.endsWith('Enabled')));
}
function taskIdentity(ctx, stacks) {
  const matches = stacks.data.resources.filter((r) => r.ResourceType === 'AWS::DMS::ReplicationTask');
  if (
    matches.length !== 1 ||
    !new RegExp(`^arn:aws:dms:${ctx.region}:${ctx.account}:task:[A-Za-z0-9]+$`).test(
      matches[0].PhysicalResourceId,
    )
  )
    throw new Error('Expected one stage-owned DMS task in the approved account and region.');
  return matches[0].PhysicalResourceId;
}
function candidates(ctx, directory) {
  return Object.fromEntries(
    names(ctx).map((name) => [
      name,
      JSON.parse(fs.readFileSync(path.join(directory, `${stackName(ctx, name)}.template.json`), 'utf8')),
    ]),
  );
}
function templateHashes(targets) {
  return Object.fromEntries(
    Object.entries(targets).map(([name, template]) => [name, hash(normalized(template))]),
  );
}
function differences(before, after, prefix = '') {
  if (hash(before) === hash(after)) return [];
  if (
    !before ||
    !after ||
    typeof before !== 'object' ||
    typeof after !== 'object' ||
    Array.isArray(before) ||
    Array.isArray(after)
  )
    return [prefix];
  return [...new Set([...Object.keys(before), ...Object.keys(after)])]
    .sort()
    .flatMap((key) => differences(before[key] ?? null, after[key] ?? null, `${prefix}/${key}`));
}
function guardChanges(stacks, targets) {
  const changes = {};
  for (const [name, live] of Object.entries(stacks)) {
    const before = normalized(live.template),
      after = normalized(targets[name]);
    changes[name] = differences(before, after);
    // Fail closed for stateful ownership and capture topology. DMS task settings are
    // mutable in place; selections/slot/endpoints/instance need a dedicated handoff.
    for (const id of new Set([...Object.keys(before.Resources), ...Object.keys(after.Resources)])) {
      const old = before.Resources[id],
        next = after.Resources[id];
      const type = old?.Type || next?.Type;
      if (
        old &&
        ['AWS::Lambda::EventSourceMapping', 'AWS::Scheduler::Schedule', 'AWS::Events::Rule'].includes(type)
      ) {
        if (
          !next ||
          hash(old.Properties?.Enabled ?? old.Properties?.State ?? null) !==
            hash(next.Properties?.Enabled ?? next.Properties?.State ?? null)
        )
          throw new Error(
            `${name}/${id} changes workload activation or retires a consumer; use the dedicated handoff procedure.`,
          );
      }
      if (
        /^AWS::(?:(?:DMS|RDS|Kinesis|SQS|Cognito)::|EC2::(?:VPC|Subnet|NatGateway)$|S3::Bucket$)/.test(
          type,
        ) &&
        hash(old ?? null) !== hash(next ?? null)
      ) {
        if (type === 'AWS::DMS::ReplicationTask' && old && next) {
          const mutable = '/Properties/ReplicationTaskSettings';
          if (differences(old, next).every((p) => p === mutable || p.startsWith(`${mutable}/`))) continue;
        }
        throw new Error(
          `${name}/${id} changes capture, durable storage or workload identity. Use the dedicated handoff/recovery procedure before a routine release.`,
        );
      }
    }
    // Indirect references/conditions may alter a task without changing its resource.
    if (
      name === 'data' &&
      (hash(before.Conditions || {}) !== hash(after.Conditions || {}) ||
        hash(before.Mappings || {}) !== hash(after.Mappings || {}) ||
        hash(before.Parameters || {}) !== hash(after.Parameters || {}))
    )
      throw new Error(
        'Data-stack conditions, mappings or parameter declarations changed; review capture identity in the exceptional procedure.',
      );
  }
  return changes;
}
function verify(ctx, stacks, targets, expectedActivation, digests, expectedRuntime = {}, api = aws) {
  for (const name of names(ctx))
    if (hash(normalized(stacks[name].template)) !== hash(normalized(targets[name])))
      throw new Error(`${name} template does not match the approved release; deployment remains incomplete.`);
  for (const name of ['initialize', 'compute'])
    if (parameters(stacks[name].stack).CommitSHA !== ctx.sha)
      throw new Error(`${name} is not at the pinned release SHA.`);
  if (hash(activation(stacks)) !== hash(expectedActivation))
    throw new Error(
      'Activation parameters changed; keep capture paused and recover the approved configuration.',
    );
  const runtime = runtimeActivation(ctx, stacks, api);
  for (const [key, value] of Object.entries(expectedRuntime))
    if (runtime[key] !== value)
      throw new Error(`Workload activation changed at ${key}; release remains incomplete.`);
  for (const [key, value] of Object.entries(digests))
    if (parameters(stacks.compute.stack)[key] !== value)
      throw new Error(`Compute image ${key} does not match the approved digest.`);
}
function deployed(ctx, name, live, targetHash, digests) {
  if (hash(normalized(live[name].template)) !== targetHash) return false;
  if (['initialize', 'compute'].includes(name) && parameters(live[name].stack).CommitSHA !== ctx.sha)
    return false;
  return (
    name !== 'compute' ||
    Object.entries(digests).every(([key, value]) => parameters(live.compute.stack)[key] === value)
  );
}
function deploy(ctx, name, token, digests, run = command) {
  const env = {
    ...process.env,
    RELEASE_ID: token,
    DEPLOY_PHASE: name,
    DEPLOY_SCOPE: ['network', 'data', 'initialize'].includes(name) ? 'foundation' : 'application',
    CONTAINER_IMAGE_DIGESTS: JSON.stringify(digests),
  };
  run('bash', ['ci/deploy-stacks.sh'], 65 * 60 * 1000, env);
}
async function settle(ctx, name, api = aws, delay = sleep) {
  const deadline = Date.now() + 30 * 60 * 1000;
  for (let i = 0; i < 180 && Date.now() < deadline; i++) {
    const s = api(ctx, 'cloudformation', 'describe-stacks', ['--stack-name', stackName(ctx, name)])
      .Stacks?.[0];
    if (HEALTHY.includes(s?.StackStatus)) return;
    if (
      !['UPDATE_IN_PROGRESS', 'UPDATE_COMPLETE_CLEANUP_IN_PROGRESS', 'CREATE_IN_PROGRESS'].includes(
        s?.StackStatus,
      )
    )
      throw new Error(`${name} did not complete successfully; inspect CloudFormation before continuation.`);
    await delay(10000);
  }
  throw new Error(`${name} did not stabilize within 30 minutes. Ownership retained.`);
}
module.exports = {
  names,
  stackName,
  parameters,
  inspect,
  snapshot,
  activation,
  runtimeActivation,
  taskIdentity,
  candidates,
  templateHashes,
  differences,
  guardChanges,
  verify,
  deployed,
  deploy,
  settle,
  normalized,
};
