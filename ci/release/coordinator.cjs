'use strict';
const { hash, command, summary } = require('./io.cjs');
const { owner, assertOwner } = require('./state.cjs');
const stacks = require('./stacks.cjs');
const dms = require('./dms.cjs');
const { plan, monitor } = require('./plan.cjs');
const { approval, canContinue } = require('./github.cjs');
class Coordinator {
  constructor(ctx, store, dependencies = {}) {
    this.ctx = ctx;
    this.store = store;
    this.ops = { stacks, dms, plan, monitor, approval, canContinue, command, ...dependencies };
  }
  get active() {
    return this.current.state.active;
  }
  save(patch) {
    assertOwner(this.current, this.ctx, this.active.id);
    this.current = this.store.write(this.current, {
      ...this.current.state,
      active: { ...this.active, ...patch, updatedAt: new Date().toISOString() },
    });
  }
  intent(phase) {
    this.budget();
    this.save({ phase, operationIntent: phase, failure: null });
  }
  done(phase, evidence = {}) {
    this.save({
      completed: [...new Set([...this.active.completed, phase])],
      evidence: { ...this.active.evidence, [phase]: { at: new Date().toISOString(), ...evidence } },
      operationIntent: null,
    });
  }
  budget() {
    if (
      this.active.pauseOwned &&
      !this.active.resumedAt &&
      Date.now() - Date.parse(this.active.pauseAt) > this.active.pauseBudgetSeconds * 1000
    )
      throw new Error(
        'Owned DMS pause exceeded its WAL budget. Inspect WAL/storage and monitoring before approving a measured budget increase; capture is not automatically resumed.',
      );
  }
  async acquire(approvedPlan, targets, digests) {
    const ctx = this.ctx;
    const stored = this.store.read();
    if (stored.etag !== approvedPlan.stateEtag)
      throw new Error('Stage ownership changed after planning. Prepare and approve a fresh plan.');
    this.ops.monitor(ctx, stored.state.active);
    const live = this.ops.stacks.inspect(ctx);
    const fresh = this.ops.plan(ctx, stored, live, targets, digests);
    if (hash(fresh) !== hash(approvedPlan))
      throw new Error(
        'Stage baseline, task, requirements or configuration changed after approval. Prepare and approve a fresh plan.',
      );
    const confirmed = this.ops.approval(ctx, approvedPlan);
    if (fresh.alreadyComplete) {
      this.ops.stacks.verify(
        ctx,
        live,
        targets,
        approvedPlan.activation,
        digests,
        approvedPlan.runtimeActivation,
      );
      summary(
        `## Release already complete\n- ${ctx.account}/${ctx.region}/${ctx.stage}: ${ctx.sha}\n- Deployed templates, SHA, digests and activation match. No deployment, migration or DMS action was repeated.`,
      );
      return false;
    }
    if (stored.state.active) this.ops.canContinue(ctx, stored.state.active);
    const active = stored.state.active || {
      id: hash({
        account: ctx.account,
        region: ctx.region,
        stage: ctx.stage,
        sha: ctx.sha,
        baseline: approvedPlan.baseline,
      }),
      kind: 'RELEASE',
      sha: ctx.sha,
      baseline: approvedPlan.baseline,
      ref: ctx.ref,
      createdAt: new Date().toISOString(),
      phase: 'PREPARED',
      completed: [],
      evidence: {},
      activation: approvedPlan.activation,
      runtimeActivation: approvedPlan.runtimeActivation,
      captureConfig: approvedPlan.captureConfig,
    };
    this.current = this.store.write(stored, {
      ...stored.state,
      active: {
        ...active,
        owner: owner(ctx),
        approvedPlan,
        approval: confirmed,
        pauseBudgetSeconds: ctx.budget,
        failure: null,
      },
    });
  }
  async pause() {
    if (this.active.completed.includes('PAUSED')) return;
    if (!this.active.dms) {
      const task = this.ops.dms.read(this.ctx, this.active.approvedPlan.task.arn);
      this.ops.dms.admission(task, null);
      this.save({
        dms: this.ops.dms.evidence(task),
        priorDmsState: task.Status,
        pauseOwned: task.Status === 'running',
        pauseAt: task.Status === 'running' ? new Date().toISOString() : null,
        phase: 'PAUSING',
        operationIntent: 'PAUSING',
      });
    }
    const stopped = await this.ops.dms.pause(this.ctx, this.active.dms);
    this.save({ phase: 'PAUSED', stopped });
    this.done('PAUSED', { checkpointHash: hash(stopped.checkpoint) });
  }
  async deploy(name, digests) {
    const phase = `DEPLOY_${name.toUpperCase()}`;
    if (this.active.completed.includes(phase)) return;
    const continuing = this.active.operationIntent === phase;
    this.intent(phase);
    // CloudFormation may still be applying an accepted operation after runner loss.
    if (continuing) await this.ops.stacks.settle(this.ctx, name);
    const previous = continuing ? this.ops.stacks.inspect(this.ctx) : null;
    if (
      !continuing ||
      !this.ops.stacks.deployed(this.ctx, name, previous, this.active.approvedPlan.targets[name], digests)
    ) {
      this.ops.stacks.deploy(this.ctx, name, this.active.id, digests, this.ops.command);
    }
    const live = this.ops.stacks.inspect(this.ctx);
    if (hash(this.ops.stacks.normalized(live[name].template)) !== this.active.approvedPlan.targets[name])
      throw new Error(`${name} deployment did not produce the approved template.`);
    this.done(phase, { stack: this.ops.stacks.snapshot(live)[name] });
  }
  async migrate() {
    if (this.active.completed.includes('MIGRATED')) return;
    this.intent('MIGRATING');
    this.ops.command('bash', ['ci/invoke-operation.sh', 'migrate'], 18 * 60 * 1000, {
      ...process.env,
      RELEASE_ID: this.active.id,
    });
    this.done('MIGRATED', { initializeSha: this.ctx.sha, result: 'ready' });
  }
  async finish(targets, digests) {
    if (!this.active.resumedAt && this.active.phase !== 'RESUMING') this.intent('VERIFY_DEPLOYMENT');
    const live = this.ops.stacks.inspect(this.ctx);
    this.ops.stacks.verify(
      this.ctx,
      live,
      targets,
      this.active.activation,
      digests,
      this.active.runtimeActivation,
    );
    this.done('DEPLOYED', { sha: this.ctx.sha, stacks: this.ops.stacks.snapshot(live) });
    if (this.active.pauseOwned) {
      if (!this.active.resumedAt) {
        const current = this.ops.dms.read(this.ctx, this.active.dms.arn);
        this.ops.dms.sameTask(current, this.active.dms);
        if (this.active.phase === 'RESUMING' && current.Status === 'running') {
          this.save({ resumedAt: new Date().toISOString(), finalDmsState: 'running' });
        } else {
          this.intent('RESUMING');
          const resumed = await this.ops.dms.resume(this.ctx, this.active.dms);
          this.save({ resumedAt: new Date().toISOString(), finalDmsState: resumed.status });
        }
      }
      this.intent('VERIFYING');
      const evidence = await this.ops.dms.health(
        this.ctx,
        this.active.stopped,
        this.active.activation.CdcRouterEnabled,
        this.active.resumedAt,
        live,
      );
      this.done('CDC_VERIFIED', evidence);
    } else {
      const task = this.ops.dms.read(this.ctx, this.active.dms.arn);
      this.ops.dms.sameTask(task, this.active.dms);
      if (task.Status !== 'stopped')
        throw new Error('Intentionally stopped DMS changed state; review before completion.');
      this.save({ finalDmsState: 'stopped' });
      this.done('CDC_INTENTIONALLY_STOPPED');
    }
    const completed = {
      ...this.active,
      phase: 'COMPLETE',
      operationIntent: null,
      completedAt: new Date().toISOString(),
      stacks: this.ops.stacks.snapshot(live),
      runtimeActivation: this.ops.stacks.runtimeActivation(this.ctx, live),
    };
    this.current = this.store.write(this.current, {
      ...this.current.state,
      active: null,
      completed,
      adoptionRequired: false,
    });
    this.report(completed);
  }
  report(release) {
    const duration = release.pauseAt
      ? Math.round(
          (Date.parse(release.resumedAt || new Date().toISOString()) - Date.parse(release.pauseAt)) / 1000,
        )
      : 0;
    summary(
      `## Release ${release.phase}\n- ${this.ctx.account}/${this.ctx.region}/${this.ctx.stage}: ${release.sha} (${release.ref})\n- Confirmed phases: ${release.completed.join(', ')}\n- Migration: ${release.evidence.MIGRATED?.result || 'not confirmed'}\n- DMS prior/final: ${release.priorDmsState || 'not touched'} / ${release.finalDmsState || 'not verified'}\n- Owned pause: ${duration}s; budget ${release.pauseBudgetSeconds}s\n- ${release.phase === 'COMPLETE' ? 'Stage ownership released.' : 'Ownership retained. Continue this exact SHA using Release; inspect docs/infra.md#release-recovery. No automatic unsafe resumption.'}`,
    );
  }
  async run(approvedPlan, targets, digests) {
    if ((await this.acquire(approvedPlan, targets, digests)) === false) return;
    try {
      await this.pause();
      for (const name of ['network', 'data', 'initialize']) await this.deploy(name, digests);
      await this.migrate();
      for (const name of ['compute', 'api', ...(this.ctx.stage === 'prod' ? ['observability'] : [])])
        await this.deploy(name, digests);
      await this.finish(targets, digests);
    } catch (error) {
      // Error messages from the closed command boundary contain no provider payloads.
      try {
        this.save({
          failure: { at: new Date().toISOString(), phase: this.active.phase, message: error.message },
        });
      } catch {
        /* Lost state-write response must not trigger another mutation. */
      }
      this.report(this.active);
      throw error;
    }
  }
}
module.exports = { Coordinator };
