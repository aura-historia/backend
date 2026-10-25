'use strict';
const { createHash } = require('node:crypto');

const PROTOCOL = '2026-07-28';

function eventFor(host, path, body, headers = {}) {
  const method = body ? 'POST' : 'GET';
  return {
    version: '2.0', routeKey: `${method} ${path}`, rawPath: path, rawQueryString: '',
    headers: { host, ...(body ? { 'content-type': 'application/json', accept: 'application/json,text/event-stream',
      'mcp-protocol-version': PROTOCOL, 'mcp-method': 'tools/call', 'mcp-name': body.params.name } : {}), ...headers },
    requestContext: { stage: '$default', requestId: 'release-read-only-probe',
      http: { method, path, protocol: 'HTTP/1.1', sourceIp: '127.0.0.1', userAgent: 'aura-release-probe' } },
    ...(body ? { body: JSON.stringify(body), isBase64Encoded: false } : {}),
  };
}

function probes(kind) {
  return kind === 'api'
    ? [{ path: '/v1/ready', status: 204 }, { path: '/v1/overview', status: 200 },
      { path: '/v1/me/notifications', status: 401 }]
    : [{ path: '/ready', status: 204 }, ...['getPublicOverview', 'simpleSearchProductListings'].map((name, i) => ({
      path: '/v1', status: 200, body: { jsonrpc: '2.0', id: i + 1, method: 'tools/call',
        params: { _meta: { 'io.modelcontextprotocol/protocolVersion': PROTOCOL,
        'io.modelcontextprotocol/clientInfo': { name: 'aura-release-probe', version: '1' },
        'io.modelcontextprotocol/clientCapabilities': {} }, name, arguments: name === 'simpleSearchProductListings'
          ? { language: 'en', currency: 'EUR', size: 1 } : {} } },
    }))];
}

function assertResponse(probe, response) {
  if (response.statusCode !== probe.status) throw new Error('Read-only release probe returned an unexpected status.');
  if (probe.body && probe.status === 200) {
    const body = JSON.parse(response.isBase64Encoded
      ? Buffer.from(response.body, 'base64').toString() : response.body);
    if (body.error || !body.result || body.result.isError) throw new Error('MCP release read failed.');
  }
}

function candidateVersion(deployment, registered, functionName) {
  const type = deployment.revision?.revisionType;
  const key = type === 'String' ? 'string' : type === 'AppSpecContent' ? 'appSpecContent' : null;
  const expected = deployment.revision?.[key]?.sha256;
  const revision = registered.revision?.[key];
  if (!key || registered.applicationName !== deployment.applicationName ||
      registered.revision?.revisionType !== type || !/^[a-f0-9]{64}$/.test(expected || '') ||
      revision?.sha256 !== expected || typeof revision.content !== 'string' ||
      createHash('sha256').update(revision.content).digest('hex') !== expected)
    throw new Error('Deployment AppSpec does not match its registered revision.');
  const spec = JSON.parse(revision.content);
  const resources = spec.Resources?.length === 1 ? Object.values(spec.Resources[0]) : [];
  const resource = resources.length === 1 ? resources[0] : null;
  const info = resource?.Properties;
  if (spec.version !== '0.0' || resource?.Type !== 'AWS::Lambda::Function' ||
      info?.Name !== functionName || info.Alias !== 'live' ||
      !/^[1-9][0-9]*$/.test(info.CurrentVersion || '') ||
      !/^[1-9][0-9]*$/.test(info.TargetVersion || ''))
    throw new Error('Deployment does not identify one expected candidate version.');
  return String(info.TargetVersion);
}

async function runHook(event, config, api, fetchRequest = fetch) {
  if (!event.DeploymentId || !event.LifecycleEventHookExecutionId) throw new Error('Invalid lifecycle event.');
  const { deploymentInfo } = await api('codedeploy', 'GetDeployment', { deploymentId: event.DeploymentId });
  if (deploymentInfo?.deploymentId !== event.DeploymentId || !deploymentInfo.applicationName)
    throw new Error('Expected the exact CodeDeploy deployment.');
  const registered = await api('codedeploy', 'GetApplicationRevision', {
    applicationName: deploymentInfo.applicationName, revision: deploymentInfo.revision,
  });
  const version = candidateVersion(deploymentInfo, registered, config.functionName);
  const isPost = event.LifecycleEventHookExecutionId.includes('AfterAllowTraffic') || config.phase === 'post';
  const samples = probes(config.kind);
  const deadline = Date.now() + 180000;
  for (let round = 0; round < 3; round++) {
    for (const probe of samples) {
      if (Date.now() >= deadline) throw new Error("Probe deadline exceeded.");
      const response = await api('lambda', 'Invoke', {
        FunctionName: config.functionName, Qualifier: version, InvocationType: 'RequestResponse',
        Payload: Buffer.from(JSON.stringify(eventFor(config.host, probe.path, probe.body))),
      });
      if (response.FunctionError || response.ExecutedVersion !== version || !response.Payload)
        throw new Error('Candidate invocation failed or executed another version.');
      assertResponse(probe, JSON.parse(Buffer.from(response.Payload).toString()));
    }
  }
  if (isPost) {
    const alias = await api('lambda', 'GetAlias', { FunctionName: config.functionName, Name: 'live' });
    if (alias.FunctionVersion !== version || Object.keys(alias.RoutingConfig?.AdditionalVersionWeights ?? {}).length)
      throw new Error('Live alias has not settled on the candidate.');
    await probePublic(config.kind, config.host, fetchRequest);
  }
  return { candidateVersion: version, samples: samples.length * 3, phase: isPost ? 'post' : 'pre' };
}

async function probePublic(kind, host, fetchRequest = fetch) {
  if (!/^[a-z0-9.-]+$/.test(host)) throw new Error('Invalid configured release probe host.');
    for (const probe of probes(kind)) {
      const response = await fetchRequest(`https://${host}${probe.path}`, {
        method: probe.body ? 'POST' : 'GET', redirect: 'error', signal: AbortSignal.timeout(20000),
        headers: { accept: 'application/json,text/event-stream', 'cache-control': 'no-cache',
          ...(probe.body ? { 'content-type': 'application/json', 'mcp-protocol-version': PROTOCOL,
            'mcp-method': 'tools/call', 'mcp-name': probe.body.params.name } : {}) },
        ...(probe.body ? { body: JSON.stringify(probe.body) } : {}),
      });
      assertResponse(probe, { statusCode: response.status, body: await response.text() });
    }
  return probes(kind).length;
}

let clients;
exports.handler = async (event) => {
  if (!clients) {
    const { CodeDeployClient, GetDeploymentCommand, GetApplicationRevisionCommand, PutLifecycleEventHookExecutionStatusCommand } = require('@aws-sdk/client-codedeploy');
    const { LambdaClient, InvokeCommand, GetAliasCommand } = require('@aws-sdk/client-lambda');
    clients = { codedeploy: new CodeDeployClient({ region: process.env.AWS_REGION }),
      lambda: new LambdaClient({ region: process.env.AWS_REGION }),
      commands: { GetDeployment: GetDeploymentCommand, GetApplicationRevision: GetApplicationRevisionCommand, PutLifecycleEventHookExecutionStatus: PutLifecycleEventHookExecutionStatusCommand,
        Invoke: InvokeCommand, GetAlias: GetAliasCommand } };
  }
  const api = (service, operation, input) => clients[service].send(new clients.commands[operation](input),
    { abortSignal: AbortSignal.timeout(25000) });
  let status = 'Failed';
  try {
    const evidence = await runHook(event, { functionName: process.env.FUNCTION_NAME,
      host: process.env.PROBE_HOST, kind: process.env.PROBE_KIND, phase: process.env.PROBE_PHASE }, api);
    console.log(JSON.stringify({ deploymentId: event.DeploymentId, status: 'PASS', ...evidence }));
    status = 'Succeeded';
  } catch {
    console.error(JSON.stringify({ deploymentId: event.DeploymentId, status: 'FAIL', category: 'READ_ONLY_PROBE_FAILED' }));
  }
  await api('codedeploy', 'PutLifecycleEventHookExecutionStatus', {
    deploymentId: event.DeploymentId, lifecycleEventHookExecutionId: event.LifecycleEventHookExecutionId, status,
  });
};
exports.runHook = runHook;
exports.candidateVersion = candidateVersion;
exports.eventFor = eventFor;

exports.probePublic = probePublic;

exports.probes = probes;
exports.assertResponse = assertResponse;
