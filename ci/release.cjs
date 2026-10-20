#!/usr/bin/env node
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { context, identity, hash, command, summary, output } = require('./release/io.cjs');
const { Store, assertOwner } = require('./release/state.cjs');
const { protection } = require('./release/github.cjs');
const { plan, render, monitor } = require('./release/plan.cjs');
const stacks = require('./release/stacks.cjs');
const dms = require('./release/dms.cjs');
const { Coordinator } = require('./release/coordinator.cjs');
const { exceptional, exceptionalPlan } = require('./release/exception.cjs');
const { publishAssets } = require('./release/assets.cjs');
const { loadCatalog, validateDigestMap } = require('./container-images.cjs');
function protocol() {
  if (JSON.parse(fs.readFileSync('releases/protocol.json', 'utf8')).version !== 1)
    throw new Error(
      'Unsupported release helper version. Historical helpers cannot bypass release ownership.',
    );
}
function assertPhase(ctx, store, phase) {
  const active = assertOwner(store.read(), ctx, process.env.RELEASE_ID);
  const allowed =
    active.kind === 'RELEASE'
      ? (phase === 'migrate' && active.phase === 'MIGRATING') ||
        active.phase === `DEPLOY_${phase.toUpperCase()}`
      : phase.toUpperCase() === active.kind ||
        (phase === 'exception' && ['FOUNDATION', 'ALL'].includes(active.kind));
  if (!allowed) throw new Error(`Operation ${phase} is not authorized by the durable release phase.`);
  if (active.kind === 'RELEASE') {
    const task = dms.read(ctx, active.dms.arn);
    dms.sameTask(task, active.dms);
    if (task.Status !== 'stopped')
      throw new Error('Deployment requires the owned task to be stably stopped.');
  }
}
async function reconcile(ctx, active) {
  if (active?.operationIntent?.startsWith('DEPLOY_')) {
    const name = active.operationIntent.slice(7).toLowerCase();
    await stacks.settle(ctx, name);
    if (name === 'data') await dms.wait(ctx, active.dms, 'stopped', ['modifying']);
  }
  if (active?.dms) {
    const current = dms.read(ctx, active.dms.arn);
    dms.admission(current, active);
    if (active.phase === 'PAUSING' && current.Status === 'stopping')
      await dms.wait(ctx, active.dms, 'stopped', ['stopping']);
    if (active.phase === 'RESUMING' && current.Status === 'starting')
      await dms.wait(ctx, active.dms, 'running', ['starting']);
  }
}
function synthesize(ctx, directory, assembly) {
  fs.mkdirSync(directory, { recursive: true });
  command(
    'npm',
    [
      '--prefix',
      'infra',
      'run',
      'cdk',
      '--',
      'synth',
      '--all',
      '--context',
      `stage=${ctx.stage}`,
      '--context',
      `account=${ctx.account}`,
      '--output',
      assembly,
      '--no-path-metadata',
      '--no-asset-metadata',
      '--no-version-reporting',
      '--no-notices',
    ],
    180000,
  );
  publishAssets(ctx, assembly);
}
async function prepare(ctx, store, kind, directory, assembly) {
  protection(ctx);
  const stored = store.read();
  await reconcile(ctx, stored.state.active);
  synthesize(ctx, directory, assembly);
  const digests = JSON.parse(process.env.CONTAINER_IMAGE_DIGESTS);
  validateDigestMap(digests, loadCatalog());
  let planned;
  if (kind === 'routine') {
    monitor(ctx, stored.state.active);
    planned = plan(ctx, stored, stacks.inspect(ctx), stacks.candidates(ctx, assembly), digests);
    summary(render(planned));
    // Repository assets are reviewed source, never private host responses.
    const diff = command('git', [
      '--no-pager',
      'diff',
      '--no-ext-diff',
      '--no-color',
      planned.baseline,
      ctx.sha,
      '--',
      'opensearch',
    ]);
    fs.writeFileSync(path.join(directory, 'opensearch.diff'), diff, { mode: 0o600 });
    if (diff) summary(`### Exact OpenSearch asset diff\n\`\`\`diff\n${diff}\n\`\`\``);
  } else {
    planned = exceptionalPlan(ctx, kind, stored);
    planned.artifacts = { digests, targets: stacks.templateHashes(stacks.candidates(ctx, assembly)) };
    summary(
      `## Exceptional ${kind} plan\n- ${ctx.account}/${ctx.region}/${ctx.stage}: ${ctx.sha}\n- Scope: ${kind}; no automatic migration or DMS lifecycle. Approval acknowledges bootstrap/rollback prerequisites and workload activation.\n- Inventory fingerprint: ${hash(planned.inventory)}`,
    );
  }
  fs.writeFileSync(path.join(directory, 'plan.json'), JSON.stringify(planned), { mode: 0o600 });
  fs.writeFileSync(path.join(directory, 'digests.json'), JSON.stringify(digests), { mode: 0o600 });
  output('plan_id', hash(planned));
  output(
    'gate',
    planned.requirements?.manual.length
      ? `Confirm OpenSearch ${planned.requirements.id.slice(0, 16)} completed and release`
      : kind === 'routine'
        ? 'Approve release'
        : `Approve exceptional ${kind}`,
  );
}
async function execute(ctx, store, directory, assembly) {
  const planned = JSON.parse(fs.readFileSync(path.join(directory, 'plan.json'), 'utf8'));
  if (hash(planned) !== process.env.RELEASE_PLAN_ID)
    throw new Error('Plan artifact does not match the reviewed plan identity.');
  process.env.RELEASE_ASSEMBLY = assembly;
  const digests = JSON.parse(fs.readFileSync(path.join(directory, 'digests.json'), 'utf8'));
  if (planned.kind) {
    const artifacts = { digests, targets: stacks.templateHashes(stacks.candidates(ctx, assembly)) };
    if (hash(artifacts) !== hash(planned.artifacts))
      throw new Error('Exceptional release artifacts differ from the approved plan.');
    process.env.CONTAINER_IMAGE_DIGESTS = JSON.stringify(digests);
    await exceptional(ctx, planned.kind, store, planned);
    return;
  }
  await new Coordinator(ctx, store).run(planned, stacks.candidates(ctx, assembly), digests);
}
async function main(args = process.argv.slice(2)) {
  const [operation, argument] = args;
  const ctx = context();
  const store = new Store(ctx);
  if (operation === 'assert-phase') return assertPhase(ctx, store, argument);
  identity(ctx);
  if (operation === 'operation') return exceptional(ctx, argument, store);
  protocol();
  const directory = path.resolve('.release');
  const assembly = path.join(directory, 'assembly');
  if (operation === 'prepare') return prepare(ctx, store, argument || 'routine', directory, assembly);
  if (operation === 'execute') return execute(ctx, store, directory, assembly);
  throw new Error(
    'Usage: release.cjs prepare [routine|foundation|all] | execute | operation migrate|initialize|adopt | assert-phase PHASE',
  );
}
if (require.main === module)
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
module.exports = { main, assertPhase, protocol };
