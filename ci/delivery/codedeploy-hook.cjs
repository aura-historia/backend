'use strict';

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
          ? { language: 'en', currency: 'EUR', pageSize: 1 } : {} } },
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

function candidateVersion(target, functionName) {
  const info = target.deploymentTarget?.lambdaTarget?.lambdaFunctionInfo;
  if (info?.functionName !== functionName || info.functionAlias !== 'live' ||
      !/^[1-9][0-9]*$/.test(String(info.targetVersion)))
    throw new Error('Deployment does not identify one expected candidate version.');
  return String(info.targetVersion);
}

async function runHook(event, config, api, fetchRequest = fetch) {
  if (!event.DeploymentId || !event.LifecycleEventHookExecutionId) throw new Error('Invalid lifecycle event.');
  const targets = await api('codedeploy', 'ListDeploymentTargets', { deploymentId: event.DeploymentId });
  if (targets.nextToken || targets.targetIds?.length !== 1) throw new Error('Expected one CodeDeploy Lambda target.');
  const target = await api('codedeploy', 'GetDeploymentTarget', { deploymentId: event.DeploymentId, targetId: targets.targetIds[0] });
  const version = candidateVersion(target, config.functionName);
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
    const { CodeDeployClient, ListDeploymentTargetsCommand, GetDeploymentTargetCommand, PutLifecycleEventHookExecutionStatusCommand } = require('@aws-sdk/client-codedeploy');
    const { LambdaClient, InvokeCommand, GetAliasCommand } = require('@aws-sdk/client-lambda');
    clients = { codedeploy: new CodeDeployClient({ region: process.env.AWS_REGION }),
      lambda: new LambdaClient({ region: process.env.AWS_REGION }),
      commands: { ListDeploymentTargets: ListDeploymentTargetsCommand, GetDeploymentTarget: GetDeploymentTargetCommand, PutLifecycleEventHookExecutionStatus: PutLifecycleEventHookExecutionStatusCommand,
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
