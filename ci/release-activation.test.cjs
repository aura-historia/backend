'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { runtimeActivation } = require('./release/activation.cjs');
const { ctx } = require('./test-support/release.cjs');

function fixture(type, properties = {}, physical = 'stage-resource') {
  return {
    compute: {
      stack: { Parameters: [] },
      template: { Resources: { Work: { Type: type, Properties: properties } } },
      resources: [{ LogicalResourceId: 'Work', ResourceType: type, PhysicalResourceId: physical }],
    },
  };
}

test('inspects rules on the default and literal partner buses with exact names', () => {
  for (const bus of [undefined, 'aws.partner/stripe.com/source']) {
    const live = fixture('AWS::Events::Rule', { EventBusName: bus });
    const result = runtimeActivation(ctx, live, (_ctx, service, operation, args) => {
      assert.equal(service, 'events');
      assert.equal(operation, 'describe-rule');
      assert.deepEqual(args, ['--name', 'stage-resource', '--event-bus-name', bus || 'default']);
      return { State: 'DISABLED' };
    });
    assert.equal(result['compute/Work'], 'DISABLED');
  }
});

test('resolves the deployed stage partner bus SSM reference without decrypting', () => {
  const live = fixture('AWS::Events::Rule', {
    EventBusName: '{{resolve:ssm:/eventbridge/dev/stripe-event-bus-name:3}}',
  });
  const calls = [];
  runtimeActivation(ctx, live, (_ctx, service, operation, args) => {
    calls.push([service, operation, args]);
    if (service === 'ssm') return { Parameter: { Type: 'String', Value: 'aws.partner/stripe.com/source' } };
    return { State: 'ENABLED' };
  });
  assert.deepEqual(calls, [
    ['ssm', 'get-parameter', ['--name', '/eventbridge/dev/stripe-event-bus-name:3']],
    [
      'events',
      'describe-rule',
      ['--name', 'stage-resource', '--event-bus-name', 'aws.partner/stripe.com/source'],
    ],
  ]);
});

test('rejects unresolved, foreign and secret bus destinations before inspecting a rule', () => {
  for (const bus of [
    { 'Fn::ImportValue': 'unresolved' },
    '{{resolve:ssm:/eventbridge/prod/stripe-event-bus-name}}',
    '{{resolve:ssm-secure:/eventbridge/dev/stripe-event-bus-name}}',
    'arn:aws:events:eu-central-1:999999999999:event-bus/foreign',
  ]) {
    assert.throws(
      () =>
        runtimeActivation(ctx, fixture('AWS::Events::Rule', { EventBusName: bus }), () =>
          assert.fail('No API call allowed'),
        ),
      /destination|identity/,
    );
  }
  assert.throws(
    () =>
      runtimeActivation(
        ctx,
        fixture('AWS::Events::Rule', {
          EventBusName: '{{resolve:ssm:/eventbridge/dev/stripe-event-bus-name}}',
        }),
        () => ({ Parameter: { Type: 'SecureString', Value: 'redacted' } }),
      ),
    /plain String/,
  );
});

test('resolves named Scheduler groups even when CloudFormation returns a plain schedule name', () => {
  for (const physical of [
    'stage-resource',
    'stage-group/stage-resource',
    `arn:aws:scheduler:${ctx.region}:${ctx.account}:schedule/stage-group/stage-resource`,
  ]) {
    const live = fixture('AWS::Scheduler::Schedule', { GroupName: { Ref: 'Group' } }, physical);
    live.compute.resources.push({
      LogicalResourceId: 'Group',
      ResourceType: 'AWS::Scheduler::ScheduleGroup',
      PhysicalResourceId: 'stage-group',
    });
    const result = runtimeActivation(ctx, live, (_ctx, service, operation, args) => {
      assert.equal(service, 'scheduler');
      assert.equal(operation, 'get-schedule');
      assert.deepEqual(args, ['--name', 'stage-resource', '--group-name', 'stage-group']);
      return { State: 'DISABLED' };
    });
    assert.equal(result['compute/Work'], 'DISABLED');
  }
});

test('default schedules and parameter-backed destinations retain their scope', () => {
  for (const group of [undefined, { Ref: 'GroupName' }]) {
    const live = fixture('AWS::Scheduler::Schedule', { GroupName: group });
    live.compute.stack.Parameters.push({ ParameterKey: 'GroupName', ParameterValue: 'parameter-group' });
    runtimeActivation(ctx, live, (_ctx, _service, _operation, args) => {
      assert.deepEqual(args, [
        '--name',
        'stage-resource',
        '--group-name',
        group ? 'parameter-group' : 'default',
      ]);
      return { State: 'ENABLED' };
    });
  }
});

test('rejects mismatched schedule groups and unstable mappings', () => {
  assert.throws(
    () =>
      runtimeActivation(ctx, fixture('AWS::Scheduler::Schedule', {}, 'other/schedule'), () =>
        assert.fail('No API call allowed'),
      ),
    /identity/,
  );
  assert.throws(
    () =>
      runtimeActivation(ctx, fixture('AWS::Lambda::EventSourceMapping'), () => ({
        State: 'Enabling',
      })),
    /Unstable workload activation/,
  );
});
