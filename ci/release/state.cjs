'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { aws, hash } = require('./io.cjs');
const VERSION = 1;
function validate(state, ctx) {
  if (
    state.version !== VERSION ||
    state.account !== ctx.account ||
    state.region !== ctx.region ||
    state.stage !== ctx.stage ||
    !Number.isSafeInteger(state.revision) ||
    state.revision < 0
  )
    throw new Error(
      'Invalid release state or unsupported helper version; recover from versioned state with the owning operator.',
    );
  if (
    state.active &&
    (!/^[a-f0-9]{40}$/.test(state.active.sha || '') ||
      !state.active.owner ||
      !state.active.phase ||
      !Array.isArray(state.active.completed))
  )
    throw new Error('Incomplete release ownership record; operator recovery required.');
  if (
    state.completed &&
    (!/^[a-f0-9]{40}$/.test(state.completed.sha || '') ||
      !state.completed.stacks ||
      !/^[a-f0-9]{64}$/.test(state.completed.captureConfig || ''))
  )
    throw new Error('Completed release baseline is malformed; use versioned evidence to recover.');
  const active = state.active;
  if (active && !['RELEASE', 'MIGRATE', 'INITIALIZE', 'FOUNDATION', 'ALL'].includes(active.kind))
    throw new Error('Unsupported persisted release operation.');
  if (active?.kind === 'RELEASE') {
    if (
      !active.approvedPlan ||
      !active.activation ||
      !active.evidence ||
      !['true', 'false'].includes(active.activation.CdcRouterEnabled)
    )
      throw new Error('Release progress is malformed.');
    if (
      active.dms &&
      (typeof active.pauseOwned !== 'boolean' ||
        !['running', 'stopped'].includes(active.priorDmsState) ||
        active.pauseOwned !== (active.priorDmsState === 'running') ||
        (active.pauseOwned && !Number.isFinite(Date.parse(active.pauseAt))))
    )
      throw new Error('DMS pause ownership is malformed; never infer intentional stopped state.');
    if (active.completed.includes('PAUSED') && (!active.dms || !active.stopped))
      throw new Error('Confirmed pause has no task/checkpoint evidence.');
  }
  return state;
}
class Store {
  constructor(ctx, api = aws) {
    this.ctx = ctx;
    this.api = api;
  }
  read() {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-state-'));
    try {
      const file = path.join(dir, 'state.json');
      const response = this.api(this.ctx, 's3api', 'get-object', [
        '--bucket',
        this.ctx.bucket,
        '--key',
        this.ctx.key,
        '--expected-bucket-owner',
        this.ctx.account,
        file,
      ]);
      if (!response.ETag) throw new Error('State object has no concurrency token.');
      return { state: validate(JSON.parse(fs.readFileSync(file, 'utf8')), this.ctx), etag: response.ETag };
    } catch (error) {
      if (error.code !== 'NoSuchKey') throw error;
      return {
        state: {
          version: VERSION,
          account: this.ctx.account,
          region: this.ctx.region,
          stage: this.ctx.stage,
          revision: 0,
          completed: null,
          active: null,
        },
        etag: null,
      };
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }
  write(previous, state) {
    const next = { ...state, revision: previous.state.revision + 1, updatedAt: new Date().toISOString() };
    validate(next, this.ctx);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-state-'));
    try {
      const file = path.join(dir, 'state.json');
      fs.writeFileSync(file, JSON.stringify(next), { mode: 0o600 });
      const response = this.api(this.ctx, 's3api', 'put-object', [
        '--bucket',
        this.ctx.bucket,
        '--key',
        this.ctx.key,
        '--expected-bucket-owner',
        this.ctx.account,
        '--body',
        file,
        '--content-type',
        'application/json',
        ...(previous.etag ? ['--if-match', previous.etag] : ['--if-none-match', '*']),
      ]);
      if (!response.ETag) throw new Error('State update has no concurrency token; reread before continuing.');
      return { state: next, etag: response.ETag };
    } catch (error) {
      // A lost response may have committed. Never retry an external operation on that assumption.
      throw new Error(
        'Release state write was rejected or its result is unknown. Rerun the same release to reconcile; ownership was not cleared.',
        { cause: error },
      );
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }
}
const owner = (ctx) => ({ repository: ctx.repository, run: ctx.run, attempt: ctx.attempt });
const sameOwner = (a, b) => hash(a) === hash(b);
function assertOwner(snapshot, ctx, releaseId) {
  const active = snapshot.state.active;
  if (!active || active.sha !== ctx.sha || active.id !== releaseId || !sameOwner(active.owner, owner(ctx)))
    throw new Error(
      'Stage ownership changed. Stop and reconcile the recorded release; no operation is authorized.',
    );
  return active;
}
function assertAvailable(state, ctx, kind) {
  if (state.active && (state.active.sha !== ctx.sha || state.active.kind !== kind.toUpperCase()))
    throw new Error(
      `Stage belongs to incomplete ${state.active.kind} ${state.active.sha} in phase ${state.active.phase}. Continue that exact operation first; see docs/infra.md#release-recovery.`,
    );
}
module.exports = { Store, VERSION, validate, owner, sameOwner, assertOwner, assertAvailable };
