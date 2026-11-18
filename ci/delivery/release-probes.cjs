'use strict';
// Read-only probes of the public API and MCP front doors after a release.

const PROTOCOL = '2026-07-28';

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

module.exports = { probes, assertResponse, probePublic };
