'use strict';
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const fs = require('node:fs');

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((k) => [k, canonical(value[k])]),
    );
  return value;
}
const hash = (value) =>
  createHash('sha256')
    .update(JSON.stringify(canonical(value)) ?? 'undefined')
    .digest('hex');
function command(program, args, timeout = 120000, env = process.env) {
  try {
    return execFileSync(program, args, {
      encoding: 'utf8',
      timeout,
      maxBuffer: 32 * 1024 * 1024,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (cause) {
    // CLI errors can contain provider responses or secret-bearing arguments.
    const error = new Error(
      `${program} ${args[0]} failed or timed out; inspect restricted service evidence.`,
      { cause },
    );
    error.code = /\b(NoSuchKey|PreconditionFailed|ConditionalRequestConflict)\b/.exec(
      String(cause.stderr),
    )?.[1];
    if (/\(ValidationError\).*Stack.*does not exist/i.test(String(cause.stderr)))
      error.code = 'StackNotFound';
    throw error;
  }
}
function context(env = process.env) {
  const {
    STAGE: stage,
    DEPLOY_COMMIT_SHA: sha,
    AWS_REGION: region,
    AWS_ACCOUNT_ID: account,
    RELEASE_STATE_BUCKET: bucket,
  } = env;
  if (
    !['dev', 'prod'].includes(stage) ||
    !/^[a-f0-9]{40}$/.test(sha || '') ||
    !/^\d{12}$/.test(account || '') ||
    region !== 'eu-central-1' ||
    !/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/.test(bucket || '')
  )
    throw new Error(
      'Set valid STAGE, DEPLOY_COMMIT_SHA, AWS_ACCOUNT_ID, AWS_REGION and RELEASE_STATE_BUCKET.',
    );
  if (
    !/^\d+$/.test(env.GITHUB_RUN_ID || '') ||
    !/^\d+$/.test(env.GITHUB_RUN_ATTEMPT || '') ||
    !/^[\w.-]+\/[\w.-]+$/.test(env.GITHUB_REPOSITORY || '')
  )
    throw new Error('Release operations require a GitHub workflow run and attempt.');
  const budget = Number(env.RELEASE_PAUSE_BUDGET_SECONDS || 10800);
  if (!Number.isInteger(budget) || budget < 300 || budget > 86400)
    throw new Error('Pause budget must be 300..86400 seconds; measure WAL capacity before increasing it.');
  return {
    stage,
    sha,
    region,
    account,
    bucket,
    budget,
    repository: env.GITHUB_REPOSITORY,
    run: env.GITHUB_RUN_ID,
    attempt: env.GITHUB_RUN_ATTEMPT,
    ref: env.RELEASE_REF || sha,
    key: `stages/${account}/${region}/${stage}.json`,
  };
}
function aws(ctx, service, operation, args = []) {
  return JSON.parse(
    command(
      'aws',
      [
        service,
        operation,
        ...args,
        '--region',
        ctx.region,
        '--output',
        'json',
        '--cli-connect-timeout',
        '10',
        '--cli-read-timeout',
        '60',
      ],
      90000,
    ) || '{}',
  );
}
function github(ctx, endpoint) {
  return JSON.parse(command('gh', ['api', `repos/${ctx.repository}/${endpoint}`], 60000));
}
function identity(ctx, api = aws) {
  if (api(ctx, 'sts', 'get-caller-identity').Account !== ctx.account)
    throw new Error('AWS account does not match the approved account.');
}
function summary(text) {
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${text}\n`);
}
function output(key, value) {
  if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
module.exports = { canonical, hash, command, context, aws, github, identity, summary, output, sleep };
