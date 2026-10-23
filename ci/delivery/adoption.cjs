'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { digest, bytesDigest } = require('./contracts.cjs');
const { validateObjects } = require('./build.cjs');
const { candidateTemplates, artifactParameters } = require('./preparation.cjs');
const {
  inspectDatabase,
  cumulativeSource,
  ledgerFindings,
} = require('./inspection.cjs');
const { snapshots } = require('./schema.cjs');
const admission = require('./admission.cjs');
const storage = require('./storage.cjs');

function parseTemplate(value) {
  return typeof value === 'string' ? JSON.parse(value) : value;
}
function resolveParameter(value, parameters) {
  if (typeof value === 'string') return value;
  if (
    value &&
    Object.keys(value).length === 1 &&
    value.Ref &&
    parameters.has(value.Ref)
  )
    return parameters.get(value.Ref);
  throw new Error('Adoption cannot resolve artifact reference');
}
async function verifyStacks(
  ctx,
  config,
  bundle,
  templates,
  api = admission.aws,
) {
  const values = artifactParameters(bundle),
    imageDigests = new Set(bundle.images.map((image) => image.digest));
  for (const name of config.stackNames) {
    const candidate = templates[name];
    if (!candidate)
      throw new Error(`Missing candidate template for baseline ${name}`);
    const actual = api(ctx, 'cloudformation', 'describe-stacks', [
      '--stack-name',
      name,
    ]).Stacks;
    if (
      actual?.length !== 1 ||
      ![
        'CREATE_COMPLETE',
        'UPDATE_COMPLETE',
        'UPDATE_ROLLBACK_COMPLETE',
        'IMPORT_COMPLETE',
      ].includes(actual[0].StackStatus)
    )
      throw new Error('Baseline stack is missing or not settled');
    const live = parseTemplate(
      api(ctx, 'cloudformation', 'get-template', ['--stack-name', name])
        .TemplateBody,
    );
    if (digest(live) !== digest(candidate.template))
      throw new Error(`Actual template differs from selected bundle: ${name}`);
    const parameters = new Map(
      (actual[0].Parameters || []).map((value) => [
        value.ParameterKey,
        value.ParameterValue,
      ]),
    );
    for (const key of Object.keys(live.Parameters || {}))
      if (
        Object.hasOwn(values, key) &&
        parameters.get(key) !== String(values[key])
      )
        throw new Error(
          `Actual artifact/source parameter differs: ${name}/${key}`,
        );
    const resources = api(ctx, 'cloudformation', 'list-stack-resources', [
      '--stack-name',
      name,
    ]).StackResourceSummaries;
    if (!Array.isArray(resources))
      throw new Error('Missing deployed resource inventory');
    const functions = new Map();
    for (const [id, resource] of Object.entries(live.Resources)) {
      if (resource.Type !== 'AWS::Lambda::Function') continue;
      const physical = resources.find(
        (value) => value.LogicalResourceId === id,
      )?.PhysicalResourceId;
      if (!physical) throw new Error('Missing deployed Lambda identity');
      const deployed = api(ctx, 'lambda', 'get-function-configuration', [
        '--function-name',
        physical,
      ]);
      let expected;
      if (resource.Properties.Code.ZipFile !== undefined) {
        const handler = resource.Properties.Handler?.split('.')[0];
        if (
          !/^[a-zA-Z0-9_-]+$/.test(handler || '') ||
          !/^nodejs/.test(resource.Properties.Runtime || '')
        )
          throw new Error('Unsupported inline baseline Lambda');
        const metadata = api(ctx, 'lambda', 'get-function', [
          '--function-name',
          physical,
        ]);
        const location = new URL(metadata.Code?.Location);
        if (
          location.protocol !== 'https:' ||
          !location.hostname.endsWith('.amazonaws.com')
        )
          throw new Error('Invalid native Lambda code location');
        const response = await fetch(location, {
          signal: AbortSignal.timeout(30000),
        });
        if (!response.ok)
          throw new Error('Cannot read actual baseline inline code');
        const bytes = Buffer.from(await response.arrayBuffer());
        expected = bytesDigest(bytes);
        const directory = fs.mkdtempSync(
          path.join(os.tmpdir(), 'delivery-adoption-'),
        );
        try {
          const filename = path.join(directory, 'function.zip');
          fs.writeFileSync(filename, bytes, { mode: 0o600 });
          const entries = execFileSync('unzip', ['-Z1', filename], {
            encoding: 'utf8',
          })
            .trim()
            .split('\n');
          if (entries.length !== 1 || entries[0] !== `${handler}.js`)
            throw new Error('Unexpected deployed inline Lambda files');
          const code = execFileSync(
            'unzip',
            ['-p', filename, `${handler}.js`],
            { maxBuffer: 2 * 1024 * 1024 },
          );
          if (!code.equals(Buffer.from(resource.Properties.Code.ZipFile)))
            throw new Error(
              'Actual inline Lambda differs from immutable template',
            );
        } finally {
          fs.rmSync(directory, { recursive: true, force: true });
        }
      } else {
        const key = resolveParameter(
          resource.Properties.Code.S3Key,
          parameters,
        );
        const version = resolveParameter(
          resource.Properties.Code.S3ObjectVersion,
          parameters,
        );
        const asset = bundle.artifacts.find(
          (value) => value.key === key && value.versionId === version,
        );
        if (!asset)
          throw new Error(
            'Deployed Lambda is outside selected immutable bundle',
          );
        expected = asset.sha256;
      }
      if (
        Buffer.from(deployed.CodeSha256 || '', 'base64').toString('hex') !==
          expected ||
        deployed.LastUpdateStatus !== 'Successful'
      )
        throw new Error('Deployed Lambda ZIP differs or is not ready');
      functions.set(id, expected);
    }
    for (const [id, resource] of Object.entries(live.Resources)) {
      if (resource.Type === 'AWS::Lambda::Alias') {
        const expected = functions.get(resource.Properties.FunctionName?.Ref);
        const physical = resources.find(
          (value) => value.LogicalResourceId === id,
        )?.PhysicalResourceId;
        if (!expected || !physical)
          throw new Error('Cannot verify baseline serving alias');
        const deployed = api(ctx, 'lambda', 'get-function-configuration', [
          '--function-name',
          physical,
        ]);
        if (
          Buffer.from(deployed.CodeSha256 || '', 'base64').toString('hex') !==
          expected
        )
          throw new Error('Serving alias differs from baseline artifact');
        const alias = api(ctx, 'lambda', 'get-alias', [
          '--function-name',
          deployed.FunctionArn.split(':').slice(0, 7).join(':'),
          '--name',
          resource.Properties.Name,
        ]);
        if (
          Object.keys(alias.RoutingConfig?.AdditionalVersionWeights || {})
            .length
        )
          throw new Error('Baseline serving alias has an active traffic split');
      }
      if (resource.Type === 'AWS::ECS::TaskDefinition') {
        const physical = resources.find(
          (value) => value.LogicalResourceId === id,
        )?.PhysicalResourceId;
        const task = api(ctx, 'ecs', 'describe-task-definition', [
          '--task-definition',
          physical,
        ]).taskDefinition;
        if (
          !task ||
          !task.containerDefinitions?.length ||
          task.containerDefinitions.some(
            (container) =>
              ![...imageDigests].some((value) =>
                container.image.endsWith(`@${value}`),
              ),
          )
        )
          throw new Error(
            'Deployed container revision differs from immutable image catalog',
          );
      }
    }
  }
}
async function adopt(ctx, config, evidence, store, options = {}) {
  const read = options.readJson || storage.readJson,
    api = options.api || admission.aws;
  const current = store.read();
  if (
    current &&
    (current.state.state !== 'READY' ||
      current.state.active ||
      current.state.lastVerified)
  )
    throw new Error(
      'Baseline adoption requires an absent or unverified idle record; it cannot replace a verified baseline or clear a hold',
    );
  if (!/^[a-f0-9]{64}$/.test(evidence.bundleDigest || '') || !evidence.reason)
    throw new Error('Explicit completed baseline bundle and reason required');
  const bundle = read(ctx.bucket, `bundles/${evidence.bundleDigest}.json`);
  if (digest(bundle) !== evidence.bundleDigest)
    throw new Error('Baseline bundle digest mismatch');
  (options.validateObjects || validateObjects)(bundle);
  const inspected = (options.inspectDatabase || inspectDatabase)(
    ctx,
    config.inspectorFunctionArn,
  );
  const observed = {
    sha256: digest(inspected.schema),
    observedAt: new Date().toISOString(),
    migrations: inspected.ledger.map((row) => ({
      version: String(row.version),
      checksum: row.checksum,
    })),
  };
  const operation = {
    executionId: `maintenance:${digest({ target: ctx.target, evidence })}`,
    requestDigest: digest(evidence),
    bundleDigest: evidence.bundleDigest,
  };
  const held = {
    version: 1,
    target: ctx.target,
    revision: (current?.state.revision || 0) + 1,
    state: 'MAINTENANCE',
    lastVerified: null,
    observedSchema: observed,
    active: operation,
    holdReason: 'Baseline adoption verification has not completed',
  };
  store.write(held, current?.etag);
  try {
    const templates = (options.candidateTemplates || candidateTemplates)(
      bundle,
      ctx.target,
    );
    await (options.verifyStacks || verifyStacks)(
      ctx,
      config,
      bundle,
      templates,
      api,
    );
    if (
      inspected.ledger.length !== bundle.migrations.length ||
      inspected.ledger.some(
        (row) =>
          !row.success ||
          !bundle.migrations.some(
            (item) =>
              item.version === String(row.version) &&
              item.checksum === row.checksum,
          ),
      )
    )
      throw new Error('Actual schema ledger differs from baseline bundle');
    const source = (options.cumulativeSource || cumulativeSource)(
      process.env.DELIVERY_REPO_DIR || '.',
      bundle.sourceSha,
      bundle.sourceSha,
    );
    if (ledgerFindings(inspected.ledger, source.migrations).length)
      throw new Error('Baseline source/ledger mismatch');
    const reconstructed = await (options.snapshots || snapshots)(
      source.migrations,
      inspected.ledger,
    );
    if (digest(reconstructed.after) !== digest(inspected.schema))
      throw new Error('Actual schema differs from reconstructed baseline');
    const native = (
      options.inspectNative || require('./maintenance.cjs').inspectManualNative
    )({ ...ctx, executionId: operation.executionId }, config.stackNames, api);
    if (!native.nativeWorkSettled)
      throw new Error('Native baseline operations are not settled');
    const raw = api(ctx, 'dms', 'describe-replication-tasks', [
      '--filters',
      `Name=replication-task-arn,Values=${config.dmsTaskArn}`,
    ]).ReplicationTasks;
    if (raw?.length !== 1 || raw[0].ReplicationTaskArn !== config.dmsTaskArn)
      throw new Error('Missing baseline capture health inventory');
    const fingerprint = require('./health.cjs').fingerprint;
    const baselineDms = {
      arn: raw[0].ReplicationTaskArn,
      status: raw[0].Status,
      settingsHash: fingerprint(raw[0].ReplicationTaskSettings),
      mappingsHash: fingerprint(raw[0].TableMappings),
    };
    const health = await (
      options.verifyCurrentHealth ||
      require('./execution.cjs').verifyCurrentHealth
    )(ctx, config, baselineDms, held.observedSchema.observedAt);
    (options.persist || storage.putImmutableJson)(
      ctx.bucket,
      `maintenance/${ctx.target}/${digest(operation)}/adoption-evidence.json`,
      {
        version: 1,
        bundleDigest: evidence.bundleDigest,
        sourceSha: bundle.sourceSha,
        observedSchema: observed,
        native,
        health,
        verifiedAt: new Date().toISOString(),
      },
    );
    const latest = store.read();
    if (!latest || digest(latest.state) !== digest(held))
      throw new Error('Baseline adoption exclusion changed');
    const ready = {
      ...held,
      revision: held.revision + 1,
      state: 'READY',
      active: null,
      holdReason: null,
      lastVerified: {
        bundleDigest: evidence.bundleDigest,
        sourceSha: bundle.sourceSha,
        verifiedAt: new Date().toISOString(),
      },
    };
    store.write(ready, latest.etag);
    return { state: ready, stateDigest: digest(ready) };
  } catch (error) {
    throw new Error('Baseline adoption unverified; maintenance hold retained', {
      cause: error,
    });
  }
}
module.exports = { verifyStacks, adopt };
