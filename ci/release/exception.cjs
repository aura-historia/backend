'use strict';
const { aws, command, hash, summary } = require('./io.cjs');
const { assertAvailable, owner } = require('./state.cjs');
const { approval, canContinue } = require('./github.cjs');
const stacks = require('./stacks.cjs');
const dms = require('./dms.cjs');
const { ancestor } = require('./requirements.cjs');
function inventory(ctx, api = aws) {
  const result = {};
  for (const name of stacks.names(ctx)) {
    try {
      const found = api(ctx, 'cloudformation', 'describe-stacks', [
        '--stack-name',
        stacks.stackName(ctx, name),
      ]).Stacks;
      if (
        found?.length !== 1 ||
        !['CREATE_COMPLETE', 'UPDATE_COMPLETE', 'UPDATE_ROLLBACK_COMPLETE', 'IMPORT_COMPLETE'].includes(
          found[0].StackStatus,
        )
      )
        throw new Error(`${name} is unhealthy; repair CloudFormation before the exceptional operation.`);
      const commit = (found[0].Parameters || []).find((p) => p.ParameterKey === 'CommitSHA');
      if (commit && !/^[a-f0-9]{40}$/.test(commit.ParameterValue || ''))
        throw new Error(`${name} has an invalid release SHA; inspect CloudFormation before proceeding.`);
      result[name] = {
        sha: commit?.ParameterValue || null,
        fingerprint: hash(found[0]),
      };
    } catch (error) {
      if (error.code !== 'StackNotFound') throw error;
      result[name] = null;
    }
  }
  return result;
}
function exceptionalPlan(ctx, kind, store) {
  assertAvailable(store.state, ctx, kind);
  return {
    version: 1,
    kind,
    account: ctx.account,
    region: ctx.region,
    stage: ctx.stage,
    sha: ctx.sha,
    run: ctx.run,
    attempt: ctx.attempt,
    stateEtag: store.etag,
    inventory: inventory(ctx),
  };
}
async function exceptional(ctx, kind, store, selectedPlan) {
  if (!['migrate', 'initialize', 'foundation', 'all', 'adopt'].includes(kind))
    throw new Error('Unknown exceptional operation.');
  const current = store.read();
  const plan = exceptionalPlan(ctx, kind, current);
  if (selectedPlan?.artifacts) plan.artifacts = selectedPlan.artifacts;
  if (selectedPlan && hash(plan) !== hash(selectedPlan))
    throw new Error('Exceptional-operation baseline changed; prepare and approve a fresh plan.');
  const confirmed = approval(ctx, plan);
  if (current.state.active) canContinue(ctx, current.state.active);
  if (kind === 'adopt') {
    if (current.state.active)
      throw new Error(
        'Adoption cannot discard an incomplete operation. Continue its recorded release or operation first.',
      );
    const live = stacks.inspect(ctx);
    for (const name of ['initialize', 'compute'])
      if (stacks.parameters(live[name].stack).CommitSHA !== ctx.sha)
        throw new Error(
          'Adoption requires exact deployed initialize and compute SHAs. Complete or reconcile the exceptional deployment first.',
        );
    if (!ancestor(ctx.sha, 'refs/remotes/origin/develop'))
      throw new Error('Baseline must be a verified commit reachable from develop.');
    const task = dms.read(ctx, stacks.taskIdentity(ctx, live));
    dms.topology(live, task);
    dms.admission(task, null);
    let health = { intentionallyStopped: true };
    if (task.Status === 'running')
      health = await dms.health(
        ctx,
        dms.evidence(task),
        stacks.activation(live).CdcRouterEnabled,
        new Date(Date.now() - 300000).toISOString(),
        live,
      );
    store.write(current, {
      ...current.state,
      completed: {
        sha: ctx.sha,
        phase: 'ADOPTED',
        completedAt: new Date().toISOString(),
        stacks: stacks.snapshot(live),
        approval: confirmed,
        health,
        captureConfig: dms.captureConfig(ctx, task),
        runtimeActivation: stacks.runtimeActivation(ctx, live),
      },
      adoptionRequired: false,
    });
    summary(
      `## Release baseline adopted\n- ${ctx.account}/${ctx.region}/${ctx.stage}: ${ctx.sha}\n- All stack/resource states, compute/initialize SHA and CDC health verified. Approval attests application and OpenSearch correspondence to this baseline.`,
    );
    return;
  }
  if (kind === 'migrate' && plan.inventory.compute && !ancestor(plan.inventory.compute.sha, ctx.sha))
    throw new Error(
      'Never invoke an older migrator for rollback. Use the current/forward initialization release.',
    );
  const id =
    current.state.active?.id ||
    hash({ kind, sha: ctx.sha, account: ctx.account, stage: ctx.stage, revision: current.state.revision });
  let owned = store.write(current, {
    ...current.state,
    active: {
      id,
      kind: kind.toUpperCase(),
      sha: ctx.sha,
      owner: owner(ctx),
      phase: kind.toUpperCase(),
      completed: [],
      operationIntent: kind.toUpperCase(),
      approvedPlan: plan,
      approval: confirmed,
      createdAt: current.state.active?.createdAt || new Date().toISOString(),
    },
  });
  try {
    if (['foundation', 'all'].includes(kind))
      command('bash', ['ci/deploy-stacks.sh'], 90 * 60 * 1000, {
        ...process.env,
        RELEASE_ID: id,
        DEPLOY_SCOPE: kind,
      });
    else
      command('bash', ['ci/invoke-operation.sh', kind], 18 * 60 * 1000, { ...process.env, RELEASE_ID: id });
    store.write(owned, {
      ...owned.state,
      active: null,
      adoptionRequired: true,
      lastException: { ...owned.state.active, completedAt: new Date().toISOString() },
    });
    summary(
      `## Exceptional ${kind} completed\n- ${ctx.stage}: ${ctx.sha}\n- Complete bootstrap/rollback requirements, then Adopt release baseline before routine Release. This operation did not manage capture.`,
    );
  } catch (error) {
    try {
      owned = store.write(owned, {
        ...owned.state,
        active: { ...owned.state.active, failure: { message: error.message, at: new Date().toISOString() } },
      });
    } catch {
      /* Retain the authoritative record on ambiguous writes. */
    }
    summary(
      `## Exceptional ${kind} incomplete\n- ${ctx.stage}: ${ctx.sha}; ownership retained. Retry the same operation/SHA after inspecting live state.`,
    );
    throw error;
  }
}
module.exports = { exceptional, exceptionalPlan, inventory };
