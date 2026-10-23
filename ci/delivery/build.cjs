'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const {
  digest,
  bytesDigest,
  validateBundle,
  assertRequestBinding,
} = require('./contracts.cjs');
const { assertFullTests } = require('./gate.cjs');
const storage = require('./storage.cjs');

const run = (command, args, options = {}) =>
  execFileSync(command, args, { stdio: 'inherit', ...options });
const json = (file) => JSON.parse(fs.readFileSync(file));
function fileTree(root) {
  if (
    !fs.lstatSync(root).isDirectory() ||
    fs.realpathSync(root) !== path.resolve(root)
  )
    throw new Error('Build tree must be a real directory');
  return fs
    .readdirSync(root, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      if (entry.isSymbolicLink())
        throw new Error('Symlinks are forbidden in published build trees');
      return entry.isDirectory()
        ? fileTree(path.join(root, entry.name))
        : [path.join(root, entry.name)];
    });
}
function validateConfig(config) {
  if (
    config.version !== 1 ||
    config.toolchain !== '1.98.0' ||
    config.cargoLambda !== '1.9.0' ||
    config.zig !== '0.14.1' ||
    config.node !== '26.0.0' ||
    config.groups !== 1 ||
    config.architecture !== 'x86_64' ||
    config.profile !== 'release' ||
    !/@sha256:[a-f0-9]{64}$/.test(config.builderImage)
  )
    throw new Error('Unsupported/unpinned build configuration');
  if (!config.targets.prod)
    throw new Error(
      'Production target must be configured independently of dev',
    );
  for (const [target, info] of Object.entries(config.targets))
    if (
      !['dev', 'prod'].includes(target) ||
      !/^\d{12}$/.test(info.account) ||
      info.region !== 'eu-central-1'
    )
      throw new Error('Invalid build target configuration');
  return config;
}
function loadBuildConfig(
  filename = path.join(__dirname, 'build-config.json'),
  env = process.env,
) {
  const config = json(filename);
  for (const info of Object.values(config.targets)) {
    if (info.account === '${AWS_ACCOUNT_ID}') info.account = env.AWS_ACCOUNT_ID;
  }
  if (env.DELIVERY_DEV_ENABLED === 'true')
    config.targets.dev = { ...config.targets.prod };
  config.controlDigest = digest(
    [
      'build.cjs',
      'compile.sh',
      'storage.cjs',
      'gate.cjs',
      'contracts.cjs',
      'contracts.schema.json',
    ].map((file) => ({
      path: file,
      sha256: bytesDigest(fs.readFileSync(path.join(__dirname, file))),
    })),
  );
  return validateConfig(config);
}
function candidateCatalog(root) {
  const lambdas = json(path.join(root, 'ci/lambda-binaries.json'));
  const images = json(path.join(root, 'ci/container-images.json'));
  if (
    !Array.isArray(lambdas) ||
    !lambdas.length ||
    lambdas.some((binary) => !/^[a-z][a-z0-9-]+$/.test(binary)) ||
    new Set(lambdas).size !== lambdas.length
  )
    throw new Error('Invalid Lambda catalog');
  for (const image of images)
    if (
      !/^[a-z][a-z0-9-]+$/.test(image.id) ||
      !/^[a-z][a-z0-9-]+$/.test(image.binary) ||
      !/^[a-z][a-z0-9-]+$/.test(image.repository) ||
      !/^src\/[a-z0-9-]+\/Dockerfile$/.test(image.dockerfile) ||
      image.platform !== 'linux/amd64'
    )
      throw new Error('Invalid image catalog');
  return { lambdas, images };
}
function validateObjects(bundle, store = storage) {
  validateBundle(bundle);
  for (const asset of bundle.artifacts) {
    const object = store.readObject(asset.bucket, asset.key, asset.versionId);
    if (
      bytesDigest(object.body) !== asset.sha256 ||
      object.body.length !== asset.size
    )
      throw new Error(`Corrupt/missing release asset ${asset.id}`);
  }
  for (const image of bundle.images) {
    const result = store.aws('ecr', 'describe-images', [
      '--repository-name',
      image.repository,
      '--image-ids',
      `imageDigest=${image.digest}`,
    ]);
    if (
      result.imageDetails?.length !== 1 ||
      result.imageDetails[0].imageDigest !== image.digest
    )
      throw new Error(`Missing immutable image ${image.id}`);
  }
  return bundle;
}
function completeBundle(bundle, bucket, store = storage) {
  validateObjects(bundle, store);
  const bundleDigest = digest(bundle);
  store.putImmutableJson(bucket, `bundles/${bundleDigest}.json`, bundle);
  store.putImmutableJson(bucket, `builds/${bundle.build.id}/complete.json`, {
    version: 1,
    bundleDigest,
    sourceSha: bundle.sourceSha,
    buildId: bundle.build.id,
  });
  return bundleDigest;
}
function dockerRun({ candidate, output, control, image, network }, args) {
  run('docker', [
    'run',
    '--rm',
    '--network',
    network,
    '--cap-drop',
    'ALL',
    '--security-opt',
    'no-new-privileges',
    '-e',
    'AWS_EC2_METADATA_DISABLED=true',
    '-e',
    'CARGO_HOME=/output/cargo',
    '-v',
    `${candidate}:/candidate:ro`,
    '-v',
    `${output}/infra-node-modules:/candidate/infra/node_modules`,
    '-v',
    `${output}/mjml-node-modules:/candidate/mjml/node_modules`,
    '-v',
    `${output}:/output`,
    '-v',
    `${control}:/control:ro`,
    '-w',
    '/candidate',
    image,
    ...args,
  ]);
}
function compile(candidate, output, config, catalog) {
  const control = path.resolve('.delivery-control', digest(config));
  fs.mkdirSync(control, { recursive: true });
  for (const directory of ['infra-node-modules', 'mjml-node-modules'])
    fs.mkdirSync(path.join(output, directory), { recursive: true });
  for (const directory of ['infra/node_modules', 'mjml/node_modules'])
    fs.mkdirSync(path.join(candidate, directory), { recursive: true });
  fs.copyFileSync(
    path.join(__dirname, 'compile.sh'),
    path.join(control, 'compile.sh'),
  );
  fs.writeFileSync(
    path.join(control, 'lambdas.txt'),
    catalog.lambdas.join('\n') + '\n',
  );
  const templates = fileTree(path.join(candidate, 'mjml'))
    .filter((file) => file.endsWith('.mjml'))
    .map((file) => path.relative(candidate, file));
  if (!templates.length) throw new Error('Empty MJML catalog');
  fs.writeFileSync(
    path.join(control, 'templates.txt'),
    templates.join('\n') + '\n',
  );
  const image = `local/aura-release-tools:${digest(config)}`;
  const dockerfile = path.join(control, 'Dockerfile');
  fs.writeFileSync(
    dockerfile,
    `FROM ${config.builderImage}\nRUN apt-get update && apt-get install -y --no-install-recommends python3-venv && rm -rf /var/lib/apt/lists/*\n`,
  );
  run('docker', ['build', '--file', dockerfile, '--tag', image, control]);
  const parameters = { candidate, output, control, image, network: 'bridge' };
  // Source build scripts run without credentials. Link-local metadata is explicitly
  // denied during dependency/tool fetches; compilation itself has no network.
  run('iptables', [
    '-I',
    'DOCKER-USER',
    '1',
    '-d',
    '169.254.0.0/16',
    '-j',
    'DROP',
  ]);
  try {
    dockerRun(parameters, ['bash', '/control/compile.sh', 'prepare']);
    dockerRun({ ...parameters, network: 'none' }, [
      'bash',
      '/control/compile.sh',
    ]);
  } finally {
    run('iptables', [
      '-D',
      'DOCKER-USER',
      '-d',
      '169.254.0.0/16',
      '-j',
      'DROP',
    ]);
  }
}
function publishImages(candidate, output, catalog, sourceSha, buildId, store) {
  return catalog.images.map((image) => {
    const account = store.aws('sts', 'get-caller-identity').Account;
    const repository = store.aws('ecr', 'describe-repositories', [
      '--repository-names',
      image.repository,
    ]).repositories?.[0];
    if (
      !repository ||
      repository.imageTagMutability !== 'IMMUTABLE' ||
      repository.imageTagMutabilityExclusionFilters?.length
    )
      throw new Error('ECR publication requires an immutable repository');
    const uri = `${account}.dkr.ecr.${process.env.AWS_REGION}.amazonaws.com/${image.repository}`;
    const password = execFileSync('aws', ['ecr', 'get-login-password'], {
      encoding: 'utf8',
    });
    run(
      'docker',
      ['login', '--username', 'AWS', '--password-stdin', uri.split('/')[0]],
      { input: password, stdio: ['pipe', 'inherit', 'inherit'] },
    );
    const tag = `build-${buildId}`;
    const listed = store.aws('ecr', 'list-images', [
      '--repository-name',
      image.repository,
      '--filter',
      'tagStatus=TAGGED',
    ]);
    const previous = listed.imageIds?.find((value) => value.imageTag === tag);
    const local = `local/${image.id}:${buildId}`;
    run('iptables', [
      '-I',
      'DOCKER-USER',
      '1',
      '-d',
      '169.254.0.0/16',
      '-j',
      'DROP',
    ]);
    try {
      if (previous) run('docker', ['pull', `${uri}@${previous.imageDigest}`]);
      else {
        run(
          'docker',
          [
            'build',
            '--platform',
            image.platform,
            '--file',
            image.dockerfile,
            '--build-arg',
            `SOURCE_REVISION=${sourceSha}`,
            '--build-arg',
            `BUILD_WORKFLOW_IDENTITY=codebuild:${process.env.CODEBUILD_BUILD_ARN}`,
            '--label',
            `org.opencontainers.image.revision=${sourceSha}`,
            '--label',
            `com.aura-historia.build-id=${buildId}`,
            '--tag',
            local,
            '.',
          ],
          { cwd: candidate },
        );
      }
      const smokeImage = previous ? `${uri}@${previous.imageDigest}` : local;
      const inspect = JSON.parse(
        execFileSync('docker', ['image', 'inspect', smokeImage], {
          encoding: 'utf8',
        }),
      )[0];
      if (
        inspect?.Config?.Labels?.['org.opencontainers.image.revision'] !==
          sourceSha ||
        inspect?.Config?.Labels?.['com.aura-historia.build-id'] !== buildId ||
        inspect.Architecture !== 'amd64'
      )
        throw new Error('Reused/built image identity mismatch');
      run('bash', [
        path.join(__dirname, '..', 'container-images', image.id, 'smoke.sh'),
        smokeImage,
      ]);
    } finally {
      run('iptables', [
        '-D',
        'DOCKER-USER',
        '-d',
        '169.254.0.0/16',
        '-j',
        'DROP',
      ]);
    }
    if (!previous) {
      run('docker', ['tag', local, `${uri}:${tag}`]);
      run('docker', ['push', `${uri}:${tag}`]);
    }
    const result = store.aws('ecr', 'describe-images', [
      '--repository-name',
      image.repository,
      '--image-ids',
      `imageTag=${tag}`,
    ]);
    const registryDigest = result.imageDetails?.[0]?.imageDigest;
    if (!/^sha256:[a-f0-9]{64}$/.test(registryDigest))
      throw new Error('Missing registry digest after publication');
    return {
      id: image.id,
      repository: image.repository,
      digest: registryDigest,
    };
  });
}
function publishAssemblyAssets(
  assembly,
  output,
  target,
  bucket,
  store,
  artifacts,
) {
  const replacements = [];
  for (const manifestFile of fileTree(assembly).filter((file) =>
    file.endsWith('.assets.json'),
  )) {
    const manifest = json(manifestFile);
    if (Object.keys(manifest.dockerImages || {}).length)
      throw new Error('Synth-time Docker bundling is forbidden');
    for (const [id, asset] of Object.entries(manifest.files || {})) {
      const source = path.resolve(assembly, asset.source.path);
      if (
        !source.startsWith(assembly + path.sep) ||
        fs.lstatSync(source).isSymbolicLink()
      )
        throw new Error('Invalid CDK asset source');
      let file = source;
      if (asset.source.packaging === 'zip') {
        file = path.join(output, `cdk-${target}-${id}.zip`);
        const entries = fileTree(source).map((entry) =>
          path.relative(source, entry),
        );
        for (const entry of entries)
          fs.utimesSync(path.join(source, entry), 315532800, 315532800);
        run('zip', ['-X', '-q', file, ...entries], { cwd: source });
      } else if (asset.source.packaging !== 'file')
        throw new Error('Unsupported CDK asset packaging');
      const reference = store.publishFile(
        bucket,
        file,
        'ASSET',
        `cdk-${target}-${id}`,
      );
      artifacts.push(reference);
      for (const destination of Object.values(asset.destinations))
        replacements.push({ from: destination.objectKey, to: reference });
    }
  }
  function rewrite(value) {
    if (!value || typeof value !== 'object') return;
    if (value.S3Key && Object.hasOwn(value, 'S3Bucket')) {
      const replacement = replacements.find(
        (item) => item.from === value.S3Key,
      );
      if (replacement)
        Object.assign(value, {
          S3Bucket: replacement.to.bucket,
          S3Key: replacement.to.key,
          S3ObjectVersion: replacement.to.versionId,
        });
    }
    for (const child of Object.values(value)) rewrite(child);
  }
  for (const template of fileTree(assembly).filter((file) =>
    file.endsWith('.template.json'),
  )) {
    const document = json(template);
    rewrite(document);
    fs.writeFileSync(template, JSON.stringify(document, null, 2) + '\n');
  }
}
function synthesize(
  candidate,
  output,
  config,
  store,
  bucket,
  artifacts,
  images,
  sourceSha,
) {
  const targets = {};
  const parameters = {
    candidate,
    output,
    control: path.resolve('.delivery-control', digest(config)),
    image: `local/aura-release-tools:${digest(config)}`,
    network: 'none',
  };
  for (const [target, info] of Object.entries(config.targets)) {
    const assembly = path.join(output, 'assemblies', target);
    fs.mkdirSync(assembly, { recursive: true });
    const imageDigests = Object.fromEntries(
      images.map((image) => [image.id, image.digest]),
    );
    const command =
      'export PATH=/output/node-v26.0.0-linux-x64/bin:$PATH; export COMMIT_SHA="$1"; export CONTAINER_IMAGE_DIGESTS="$2"; cd infra; npm run synth -- --context stage="$3" --context account="$4" --context region="$5" --output "/output/assemblies/$3" --lookups false';
    dockerRun(parameters, [
      'bash',
      '-c',
      command,
      'synth',
      sourceSha,
      JSON.stringify(imageDigests),
      target,
      info.account,
      info.region,
    ]);
    publishAssemblyAssets(assembly, output, target, bucket, store, artifacts);
    const templates = fileTree(assembly).filter((file) =>
      file.endsWith('.template.json'),
    );
    if (!templates.length) throw new Error('Synthesis produced no templates');
    const templateDigests = Object.fromEntries(
      templates.map((file) => [
        path.basename(file),
        bytesDigest(fs.readFileSync(file)),
      ]),
    );
    // The assembly is an artifact; promotion never invokes candidate synth code.
    const archive = path.join(output, `assembly-${target}.tar`);
    run('tar', [
      '--sort=name',
      '--mtime=@315532800',
      '--owner=0',
      '--group=0',
      '--numeric-owner',
      '-cf',
      archive,
      '-C',
      assembly,
      '.',
    ]);
    const artifact = store.publishFile(
      bucket,
      archive,
      'ASSEMBLY',
      `assembly-${target}`,
    );
    artifacts.push(artifact);
    for (const file of templates)
      artifacts.push(
        store.publishFile(
          bucket,
          file,
          'ASSET',
          `template-${target}-${path.basename(file)}`,
        ),
      );
    targets[target] = {
      ...info,
      assemblyDigest: artifact.sha256,
      templateDigests,
    };
  }
  return targets;
}
async function main() {
  const store = storage,
    started = Date.now(),
    bucket = process.env.DELIVERY_BUCKET;
  for (const name of [
    'DELIVERY_BUCKET',
    'DELIVERY_REQUEST_KEY',
    'DELIVERY_REQUEST_VERSION',
    'DELIVERY_REQUEST_DIGEST',
    'DELIVERY_CANDIDATE_DIR',
    'DELIVERY_SOURCE_SHA',
  ])
    if (!process.env[name]) throw new Error(`Missing ${name}`);
  const request = store.readJson(
    bucket,
    process.env.DELIVERY_REQUEST_KEY,
    process.env.DELIVERY_REQUEST_VERSION,
  );
  if (
    digest(request) !== process.env.DELIVERY_REQUEST_DIGEST ||
    request.authorization.scope !== 'BUILD'
  )
    throw new Error('Missing exact approved build request');
  const tests = store.readJson(
    bucket,
    `${process.env.DELIVERY_REQUEST_KEY.slice(0, -5)}/tests.json`,
  );
  assertFullTests(tests, request);
  assertRequestBinding(request, tests);
  if (process.env.DELIVERY_SOURCE_SHA !== request.sourceSha)
    throw new Error(
      'Actual CodeConnections checkout differs from approved SHA',
    );
  const config = loadBuildConfig(process.env.DELIVERY_BUILD_CONFIG);
  const configDigest = digest(config);
  if (request.buildConfigDigest !== configDigest)
    throw new Error('Approved build configuration changed');
  const buildId = digest({ sourceSha: request.sourceSha, configDigest });
  // Reconcile a completed build; absence alone permits work, never corruption or denial.
  const listed = store.aws('s3api', 'list-objects-v2', [
    '--bucket',
    bucket,
    '--prefix',
    `builds/${buildId}/complete.json`,
  ]);
  if (
    listed.Contents?.some(
      (object) => object.Key === `builds/${buildId}/complete.json`,
    )
  ) {
    const complete = store.readJson(bucket, `builds/${buildId}/complete.json`);
    const existing = store.readJson(
      bucket,
      `bundles/${complete.bundleDigest}.json`,
    );
    if (digest(existing) !== complete.bundleDigest)
      throw new Error('Corrupt completion marker');
    validateObjects(existing, store);
    console.log(JSON.stringify(complete));
    return;
  }
  const candidate = path.resolve(process.env.DELIVERY_CANDIDATE_DIR),
    output = path.resolve('.delivery-build', buildId);
  fs.mkdirSync(output, { recursive: true });
  const catalog = candidateCatalog(candidate);
  const sourceArchive = path.join(output, 'source.tar');
  run('tar', [
    '--sort=name',
    '--mtime=@315532800',
    '--owner=0',
    '--group=0',
    '--numeric-owner',
    '--exclude=node_modules',
    '--exclude=target',
    '--exclude=.git',
    '-cf',
    sourceArchive,
    '-C',
    candidate,
    '.',
  ]);
  compile(candidate, output, config, catalog);
  const artifacts = catalog.lambdas.map((id) =>
    store.publishFile(
      bucket,
      path.join(output, 'lambdas', `${id}.zip`),
      'LAMBDA',
      id,
    ),
  );
  for (const file of fileTree(path.join(output, 'templates'))) {
    const id = path.relative(path.join(output, 'templates'), file);
    artifacts.push(store.publishFile(bucket, file, 'TEMPLATE', id));
    store.immutablePut(bucket, `mail/${buildId}/${id}`, file);
  }
  artifacts.push(store.publishFile(bucket, sourceArchive, 'SOURCE', 'source'));
  const images = publishImages(
    candidate,
    output,
    catalog,
    request.sourceSha,
    buildId,
    store,
  );
  const targets = synthesize(
    candidate,
    output,
    config,
    store,
    bucket,
    artifacts,
    images,
    request.sourceSha,
  );
  const migrationFiles = fileTree(path.join(candidate, 'migrations')).filter(
    (file) => /\/\d+_[^/]+\.sql$/.test(file),
  );
  const migrations = migrationFiles.map((file) => ({
    version: path.basename(file).split('_')[0],
    checksum: createHash('sha384').update(fs.readFileSync(file)).digest('hex'),
  }));
  const sourceAssets = [
    ...migrationFiles,
    ...fileTree(path.join(candidate, 'opensearch')),
  ].map((file) => ({
    path: path.relative(candidate, file),
    sha256: bytesDigest(fs.readFileSync(file)),
  }));
  const bundle = {
    version: 1,
    repository: request.repository,
    sourceSha: request.sourceSha,
    build: {
      id: buildId,
      configDigest,
      toolchain: config.toolchain,
      image: config.builderImage,
      profile: config.profile,
      architecture: config.architecture,
    },
    artifacts,
    images,
    catalogs: {
      lambdas: catalog.lambdas,
      containers: catalog.images.map((image) => image.id),
    },
    migrations,
    sourceAssets,
    targets,
    tests,
  };
  const bundleDigest = completeBundle(bundle, bucket, store);
  const elapsedSeconds = (Date.now() - started) / 1000;
  const peak = (phase) => {
    const filename = path.join(output, `${phase}.memory-peak-bytes`);
    if (!fs.existsSync(filename)) return null;
    const value = Number(fs.readFileSync(filename, 'utf8').trim());
    return Number.isSafeInteger(value) && value > 0 ? value : null;
  };
  const metrics = {
    buildId,
    bundleDigest,
    codeBuildId: process.env.CODEBUILD_BUILD_ID,
    groups: config.groups,
    pipelineExecutionId: process.env.DELIVERY_EXECUTION_ID || null,
    codeBuildJobs: 1,
    elapsedSeconds,
    computeMinutesObserved: elapsedSeconds / 60,
    memoryPeakBytes: { prepare: peak('prepare'), compile: peak('compile') },
    memoryScope:
      'Container cgroup peak where exposed; null means unavailable, not zero. Docker image builder memory and final billed compute minutes require native CodeBuild evidence.',
    computeChoice:
      'One shared build group is provisional until an approved cold/warm benchmark records duration and peak memory.',
    artifactBytes: artifacts.reduce((sum, asset) => sum + asset.size, 0),
    cache: 'build-identity-scoped local target; no cross-source cache claims',
  };
  store.putImmutableJson(bucket, `builds/${buildId}/metrics.json`, metrics);
  console.log(JSON.stringify(metrics));
}
if (require.main === module)
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
module.exports = {
  validateConfig,
  loadBuildConfig,
  candidateCatalog,
  validateObjects,
  completeBundle,
};
