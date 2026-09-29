'use strict';

const { execFileSync } = require('node:child_process');
const { appendFileSync } = require('node:fs');

function validateReleaseTag(tag) {
  if (!/^\d{8}-\d{4}$/.test(tag)) throw new Error('Production requires a YYYYMMDD-HHMM release tag (UTC).');
  const iso = `${tag.slice(0, 4)}-${tag.slice(4, 6)}-${tag.slice(6, 8)}T${tag.slice(9, 11)}:${tag.slice(11, 13)}:00.000Z`;
  const date = new Date(iso);
  if (!Number.isFinite(date.valueOf()) || date.toISOString() !== iso) throw new Error('Release tag must contain a valid UTC date and time.');
  return tag;
}

function resolveSource(env, git) {
  if (!['dev', 'prod'].includes(env.STAGE)) throw new Error('Stage must be dev or prod.');
  const manual = env.GITHUB_EVENT_NAME === 'workflow_dispatch';
  if (!manual && env.GITHUB_EVENT_NAME !== 'push') throw new Error('Unsupported deployment event.');
  let ref;
  if (env.STAGE === 'prod') {
    const tag = manual ? env.REQUESTED_REF : env.GITHUB_REF?.replace(/^refs\/tags\//, '');
    validateReleaseTag(tag ?? '');
    if (!manual && env.GITHUB_REF !== `refs/tags/${tag}`) throw new Error('Production pushes must select a release tag.');
    ref = `refs/tags/${tag}`;
  } else if (manual) {
    const requested = env.REQUESTED_REF || 'develop';
    if (requested !== 'develop' && !/^[0-9a-f]{40}$/.test(requested)) throw new Error('Dev ref must be develop or a full lowercase commit SHA.');
    ref = requested === 'develop' ? 'refs/remotes/origin/develop' : requested;
  } else {
    if (env.GITHUB_REF !== 'refs/heads/develop') throw new Error('Dev pushes must come from develop.');
    if (!/^[0-9a-f]{40}$/.test(env.GITHUB_SHA ?? '')) throw new Error('Invalid push commit SHA.');
    ref = env.GITHUB_SHA;
  }
  const sha = git(['rev-parse', '--verify', `${ref}^{commit}`]).trim();
  if (!/^[0-9a-f]{40}$/.test(sha)) throw new Error('Could not resolve the deployment commit.');
  if (!manual && sha !== git(['rev-parse', '--verify', `${env.GITHUB_SHA}^{commit}`]).trim()) {
    throw new Error('Release tag no longer matches the triggering commit.');
  }
  git(['merge-base', '--is-ancestor', sha, 'refs/remotes/origin/develop']);
  return { sha, ref };
}

if (require.main === module) {
  try {
    const result = resolveSource(process.env, (args) => execFileSync('git', ['--no-pager', ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }));
    appendFileSync(process.env.GITHUB_OUTPUT, `commit_sha=${result.sha}\n`);
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `## Deployment source\n- Stage: ${process.env.STAGE}\n- Ref: ${result.ref}\n- Commit: ${result.sha}\n`);
  } catch (error) {
    console.error(`Release validation failed: ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = { validateReleaseTag, resolveSource };
