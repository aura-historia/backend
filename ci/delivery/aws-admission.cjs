'use strict';

const { createHash } = require('node:crypto');
const { validateRequest, assertRequestBinding, digest } = require('./contracts.cjs');
const { assertFullTests } = require('./gate.cjs');

async function admit(event, config, api) {
  const job = event['CodePipeline.job'];
  const values = JSON.parse(job?.data?.actionConfiguration?.configuration?.UserParameters ?? '{}');
  if (!job?.id || !/^[a-f0-9]{64}$/.test(values.RequestDigest ?? '') || !values.RequestVersion ||
      !/^requests\/[A-Za-z0-9._/-]+\.json$/.test(values.RequestKey ?? '')) throw new Error('Missing authorized immutable request.');
  const response = await api('s3', 'GetObject', { Bucket: config.bucket, Key: values.RequestKey, VersionId: values.RequestVersion });
  const request = validateRequest(JSON.parse(await response.Body.transformToString()));
  if (values.ControlVersion !== config.controlVersion || request.workflowRevision !== config.workflowRevision ||
      !values.RequestKey.startsWith(`requests/${config.scope === 'BUILD' ? 'build' : config.target}/`))
    throw new Error('Unapproved control-plane revision or requester namespace.');
  const tests = await api('s3', 'GetObject', { Bucket: config.bucket, Key: `${values.RequestKey.slice(0, -5)}/tests.json` });
  const testEvidence = JSON.parse(await tests.Body.transformToString());
  assertRequestBinding(request, testEvidence);
  assertFullTests(testEvidence, request);
  if (request.buildConfigDigest !== config.buildConfigDigest) throw new Error("Unapproved release build configuration.");
  if ((config.scope === 'BUILD' && values.SourceSha !== request.sourceSha) ||
      digest(request) !== values.RequestDigest || request.repository !== config.repository ||
      request.authorization.scope !== config.scope || request.authorization.environment !== config.environment ||
      (config.target && request.target !== config.target)) throw new Error('Request approval binding is invalid.');
  // Lambda action events omit pipelineContext; this reserved variable is resolved by the native pipeline.
  const executionId = values.ExecutionId;
  if (!executionId) throw new Error('Native execution identity is missing.');
  const claimKey = `claims/${config.scope.toLowerCase()}/${request.id}.json`;
  const claim = { requestDigest: values.RequestDigest, executionId };
  try {
    await api('s3', 'PutObject', { Bucket: config.bucket, Key: claimKey,
      Body: JSON.stringify(claim), ContentType: 'application/json', IfNoneMatch: '*' });
  } catch (error) {
    if (!['PreconditionFailed', 'ConditionalRequestConflict'].includes(error.name)) throw error;
    const existing = await api('s3', 'GetObject', { Bucket: config.bucket, Key: claimKey });
    if (JSON.stringify(JSON.parse(await existing.Body.transformToString())) !== JSON.stringify(claim))
      throw new Error('Approved request is already owned by another execution.');
  }
  return { requestId: request.id, sourceSha: request.sourceSha, requestDigest: values.RequestDigest,
    admissionDigest: createHash('sha256').update(`${request.id}:${executionId}`).digest('hex') };
}

let clients;
exports.handler = async (event) => {
  if (!clients) {
    const { S3Client, GetObjectCommand, PutObjectCommand } = require('@aws-sdk/client-s3');
    const { CodePipelineClient, PutJobSuccessResultCommand, PutJobFailureResultCommand } = require('@aws-sdk/client-codepipeline');
    clients = { s3: new S3Client({ region: process.env.AWS_REGION }), codepipeline: new CodePipelineClient({ region: process.env.AWS_REGION }),
      commands: { GetObject: GetObjectCommand, PutObject: PutObjectCommand,
        PutJobSuccessResult: PutJobSuccessResultCommand, PutJobFailureResult: PutJobFailureResultCommand } };
  }
  const api = (service, operation, input) => clients[service].send(new clients.commands[operation](input));
  const jobId = event['CodePipeline.job']?.id;
  try {
    const evidence = await admit(event, { bucket: process.env.DELIVERY_BUCKET, repository: process.env.DELIVERY_REPOSITORY,
      buildConfigDigest: process.env.DELIVERY_BUILD_CONFIG_DIGEST,
      controlVersion: process.env.DELIVERY_CONTROL_VERSION, workflowRevision: process.env.DELIVERY_WORKFLOW_REVISION,
      scope: process.env.APPROVAL_SCOPE, environment: process.env.APPROVAL_ENVIRONMENT, target: process.env.DELIVERY_TARGET }, api);
    await api('codepipeline', 'PutJobSuccessResult', { jobId, outputVariables: evidence });
  } catch {
    if (!jobId) throw new Error('Native pipeline job identity is missing.');
    await api('codepipeline', 'PutJobFailureResult', { jobId,
      failureDetails: { type: 'ConfigurationError', message: 'No valid, unused protected-environment approval request.' } });
  }
};
exports.admit = admit;
