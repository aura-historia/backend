'use strict';
const { aws } = require('./io.cjs');

function destination(ctx, stack, value, type, api) {
  if (value === undefined) return 'default';
  if (value?.Ref && Object.keys(value).length === 1) {
    const resource = stack.resources.find(
      (r) => r.LogicalResourceId === value.Ref && r.ResourceType === type,
    );
    value =
      resource?.PhysicalResourceId ??
      stack.stack.Parameters?.find((p) => p.ParameterKey === value.Ref)?.ParameterValue;
  }
  if (typeof value !== 'string' || !value) throw new Error('Unresolved workload destination.');
  if (value.startsWith('{{')) {
    // Only the stage's nonsecret partner-bus configuration is read; never decrypt.
    const match = /^\{\{resolve:ssm:(\/eventbridge\/(?:dev|prod)\/[A-Za-z0-9_.-]+)(?::([0-9]+))?\}\}$/.exec(
      value,
    );
    if (!match || !match[1].startsWith(`/eventbridge/${ctx.stage}/`))
      throw new Error('Unsupported workload destination reference.');
    const name = match[1] + (match[2] ? `:${match[2]}` : '');
    const parameter = api(ctx, 'ssm', 'get-parameter', ['--name', name]).Parameter;
    if (parameter?.Type !== 'String' || typeof parameter.Value !== 'string')
      throw new Error('Partner event bus must be a plain String parameter.');
    value = parameter.Value;
  }
  return value;
}

function ruleState(ctx, stack, resource, properties, api) {
  const bus = destination(ctx, stack, properties.EventBusName, 'AWS::Events::EventBus', api);
  const prefix = `arn:aws:events:${ctx.region}:${ctx.account}:event-bus/`;
  const busName = bus.startsWith(prefix) ? bus.slice(prefix.length) : bus;
  // CloudFormation encodes rules on custom buses as bus|rule, while Ref returns a rule name.
  const parts = resource.PhysicalResourceId.split('|');
  const ruleName = parts.at(-1);
  const physicalBus = parts[0].startsWith(prefix) ? parts[0].slice(prefix.length) : parts[0];
  if (
    !/^[/.\-_A-Za-z0-9]+$/.test(busName) ||
    !/^[.\-_A-Za-z0-9]{1,64}$/.test(ruleName) ||
    parts.length > 2 ||
    (parts.length === 2 && physicalBus !== busName)
  )
    throw new Error('Invalid or foreign EventBridge rule identity.');
  return api(ctx, 'events', 'describe-rule', ['--name', ruleName, '--event-bus-name', bus]).State;
}

function scheduleState(ctx, stack, resource, properties, api) {
  const group = destination(ctx, stack, properties.GroupName, 'AWS::Scheduler::ScheduleGroup', api);
  const prefix = `arn:aws:scheduler:${ctx.region}:${ctx.account}:schedule/`;
  const physical = resource.PhysicalResourceId;
  const id = physical.startsWith(prefix) ? physical.slice(prefix.length) : physical;
  const parts = id.split('/');
  const name = parts.at(-1);
  if (
    !/^[.\-_A-Za-z0-9]{1,64}$/.test(name) ||
    !/^[.\-_A-Za-z0-9]{1,64}$/.test(group) ||
    (parts.length !== 1 && (parts.length !== 2 || parts[0] !== group))
  )
    throw new Error('Invalid or foreign Scheduler identity.');
  return api(ctx, 'scheduler', 'get-schedule', ['--name', name, '--group-name', group]).State;
}

function runtimeActivation(ctx, live, api = aws) {
  const states = {};
  for (const [name, stack] of Object.entries(live))
    for (const resource of stack.resources) {
      const properties = stack.template.Resources[resource.LogicalResourceId]?.Properties || {};
      let state;
      if (resource.ResourceType === 'AWS::Lambda::EventSourceMapping')
        state = api(ctx, 'lambda', 'get-event-source-mapping', ['--uuid', resource.PhysicalResourceId]).State;
      else if (resource.ResourceType === 'AWS::Scheduler::Schedule')
        state = scheduleState(ctx, stack, resource, properties, api);
      else if (resource.ResourceType === 'AWS::Events::Rule')
        state = ruleState(ctx, stack, resource, properties, api);
      else continue;
      if (
        ![
          'Enabled',
          'Disabled',
          'ENABLED',
          'DISABLED',
          'ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS',
        ].includes(state)
      )
        throw new Error(`Unstable workload activation at ${name}/${resource.LogicalResourceId}.`);
      states[`${name}/${resource.LogicalResourceId}`] = state;
    }
  return states;
}
module.exports = { runtimeActivation };
