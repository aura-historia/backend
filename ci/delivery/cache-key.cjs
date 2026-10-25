'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { digest, bytesDigest } = require('./contracts.cjs');

function cacheIdentity(candidate, config) {
  return digest({
    version: 1,
    config,
    inputs: ['Cargo.lock', 'ci/lambda-binaries.json'].map((file) => ({
      path: file,
      sha256: bytesDigest(fs.readFileSync(path.join(candidate, file))),
    })),
  });
}
if (require.main === module) {
  const { loadBuildConfig } = require('./build.cjs');
  // CodeBuild tokenizes cache-key commands without shell quote removal.
  console.log(cacheIdentity(process.env.CODEBUILD_SRC_DIR_Candidate,
    loadBuildConfig(process.env.DELIVERY_BUILD_CONFIG)));
}
module.exports = { cacheIdentity };
