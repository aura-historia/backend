'use strict';
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { command, aws } = require('./io.cjs');
function publishAssets(ctx, assembly, api = aws, run = command) {
  const cloudAssembly = JSON.parse(fs.readFileSync(path.join(assembly, 'manifest.json'), 'utf8'));
  for (const artifact of Object.values(cloudAssembly.artifacts)) {
    if (artifact.type !== 'aws:cloudformation:stack') continue;
    const environment = /^aws:\/\/([^/]+)\/([^/]+)$/.exec(artifact.environment);
    if (
      !environment ||
      ![ctx.account, 'unknown-account'].includes(environment[1]) ||
      ![ctx.region, 'unknown-region'].includes(environment[2])
    )
      throw new Error('Prepared CDK stack targets another account or region.');
  }
  for (const name of fs.readdirSync(assembly).filter((f) => f.endsWith('.assets.json'))) {
    const manifest = JSON.parse(fs.readFileSync(path.join(assembly, name), 'utf8'));
    if (Object.keys(manifest.dockerImages || {}).length)
      throw new Error(
        'Uncatalogued Docker assets are unsupported; publish through the immutable image catalog.',
      );
    for (const asset of Object.values(manifest.files || {})) {
      const source = path.resolve(assembly, asset.source.path);
      if (!source.startsWith(`${path.resolve(assembly)}/`))
        throw new Error('CDK asset escapes the prepared assembly.');
      for (const destination of Object.values(asset.destinations)) {
        // Environment-agnostic assemblies resolve against the checked CI identity.
        if ((destination.region ?? ctx.region) !== ctx.region || destination.assumeRoleArn)
          throw new Error('CDK asset targets an unexpected region or role.');
        const exists = run('bash', [
          'ci/s3-artifact-exists.sh',
          destination.bucketName,
          destination.objectKey,
        ]).trim();
        if (exists === 'true') continue;
        if (exists !== 'false') throw new Error('Could not establish CDK asset publication state.');
        const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'release-asset-'));
        try {
          let body = source;
          if (asset.source.packaging === 'zip') {
            body = path.join(temporary, 'asset.zip');
            // zip takes a working directory; shell positional arguments avoid interpolation.
            run('bash', ['-c', 'cd "$1" && zip -qr "$2" .', 'release-asset', source, body]);
          } else if (asset.source.packaging !== 'file') throw new Error('Unsupported CDK asset packaging.');
          api(ctx, 's3api', 'put-object', [
            '--bucket',
            destination.bucketName,
            '--expected-bucket-owner',
            ctx.account,
            '--key',
            destination.objectKey,
            '--body',
            body,
            '--if-none-match',
            '*',
          ]);
        } finally {
          fs.rmSync(temporary, { recursive: true, force: true });
        }
      }
    }
  }
}
module.exports = { publishAssets };
