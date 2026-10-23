'use strict';
const { createHash } = require('node:crypto');
const schema = require('./contracts.schema.json');

function canonicalize(value) {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') return JSON.stringify(value);
  if (typeof value === 'number' && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalize).join(',')}]`;
  if (value && Object.getPrototypeOf(value) === Object.prototype) {
    return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalize(value[key])}`).join(',')}}`;
  }
  throw new Error('Only finite JSON values can be hashed');
}
const digest = value => createHash('sha256').update(canonicalize(value)).digest('hex');
const bytesDigest = value => createHash('sha256').update(value).digest('hex');
const fail = (path, message) => { throw new Error(`${path}: ${message}`); };

// The checked-in schema is the single owner of wire shapes. This deliberately supports
// only the JSON Schema keywords used here, rather than a second validation dependency.
function validateShape(rule, value, path) {
  if (rule.$ref) return validateShape(schema.$defs[rule.$ref.split('/').pop()], value, path);
  if (rule.anyOf) {
    if (!rule.anyOf.some(alternative => { try { validateShape(alternative, value, path); return true; } catch { return false; } })) fail(path, 'no valid alternative');
    return;
  }
  if (rule.const !== undefined && value !== rule.const) fail(path, `expected ${rule.const}`);
  if (rule.enum && !rule.enum.includes(value)) fail(path, 'unknown value');
  if (rule.type) {
    const valid = {null: value === null, string: typeof value === 'string', integer: Number.isSafeInteger(value), array: Array.isArray(value), object: value !== null && typeof value === 'object' && !Array.isArray(value)}[rule.type];
    if (!valid) fail(path, `expected ${rule.type}`);
  }
  if (typeof value === 'string') {
    if (rule.minLength && value.length < rule.minLength) fail(path, 'empty value');
    if (rule.pattern && !new RegExp(rule.pattern).test(value)) fail(path, 'invalid format');
    if (rule.format === 'date-time' && (!/^\d{4}-\d{2}-\d{2}T/.test(value) || !Number.isFinite(Date.parse(value)))) fail(path, 'invalid timestamp');
  }
  if (rule.minimum !== undefined && value < rule.minimum) fail(path, 'below minimum');
  if (Array.isArray(value)) {
    if (value.length < (rule.minItems || 0)) fail(path, 'incomplete list');
    value.forEach((entry, index) => validateShape(rule.items, entry, `${path}[${index}]`));
  }
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    if (Object.keys(value).length < (rule.minProperties || 0)) fail(path, 'empty object');
    for (const required of rule.required || []) if (!Object.hasOwn(value, required)) fail(path, `missing ${required}`);
    for (const [key, entry] of Object.entries(value)) {
      const property = rule.properties?.[key] || rule.additionalProperties;
      if (!property) fail(path, `unknown field ${key}`);
      if (property !== true) validateShape(property, entry, `${path}.${key}`);
    }
  }
}
const shape = (name, value) => { validateShape(schema.$defs[name], value, name); return value; };
function unique(values, name) { if (new Set(values).size !== values.length) fail(name, 'duplicate identifiers'); }
function sameSet(actual, expected, name) {
  unique(actual, name); unique(expected, name);
  if (canonicalize([...actual].sort()) !== canonicalize([...expected].sort())) fail(name, 'catalog is incomplete or inconsistent');
}
function validateTestEvidence(evidence, expected = {}) {
  shape('testEvidence', evidence);
  sameSet(Object.keys(evidence.jobResults), evidence.requiredJobs, 'testEvidence.jobs');
  if (Object.values(evidence.jobResults).some(result => result !== 'success')) fail('testEvidence', 'all required jobs must succeed');
  for (const field of ['sourceSha','repository','workflowRevision','runId','runAttempt','inventoryDigest']) {
    if (expected[field] !== undefined && evidence[field] !== expected[field]) fail('testEvidence', `${field} does not match request`);
  }
  return evidence;
}
function validateBundle(bundle, catalogs = bundle.catalogs) {
  shape('bundle', bundle); validateTestEvidence(bundle.tests, bundle);
  unique(bundle.artifacts.map(asset => asset.id), 'bundle.artifacts');
  unique(bundle.migrations.map(migration => migration.version), 'bundle.migrations');
  unique(bundle.sourceAssets.map(asset => asset.path), 'bundle.sourceAssets');
  sameSet(bundle.artifacts.filter(asset => asset.kind === 'LAMBDA').map(asset => asset.id), catalogs.lambdas, 'bundle.lambdas');
  sameSet(bundle.images.map(image => image.id), catalogs.containers, 'bundle.containers');
  sameSet(bundle.catalogs.lambdas, catalogs.lambdas, 'bundle.catalogs.lambdas');
  sameSet(bundle.catalogs.containers, catalogs.containers, 'bundle.catalogs.containers');
  for (const [target, info] of Object.entries(bundle.targets)) {
    if (!['dev','prod'].includes(target)) fail('bundle.targets', 'unsupported target');
    if (!bundle.artifacts.some(asset => asset.id === `assembly-${target}` && asset.kind === 'ASSEMBLY' && asset.sha256 === info.assemblyDigest)) fail('bundle.targets', 'missing assembly');
  }
  for (const asset of bundle.artifacts) {
    if (!asset.key.includes(asset.sha256) || asset.versionId === 'null') fail('bundle.artifacts', 'object must be content addressed and versioned');
  }
  if (bundle.build.id !== digest({sourceSha:bundle.sourceSha, configDigest:bundle.build.configDigest})) fail('bundle.build.id', 'build identity does not bind source/config');
  return bundle;
}
function validateRequest(request) {
  shape('request', request);
  const expected = request.authorization.scope === 'BUILD' ? 'aws-release-build' : `aws-${request.target}`;
  if (request.authorization.environment !== expected) fail('request.authorization', 'approval scope/target mismatch');
  if (request.authorization.scope === 'DEPLOY' && !request.bundleDigest) fail('request', 'deployment requires completed bundle');
  return request;
}
function assertRequestBinding(request, evidence, bundle) {
  validateRequest(request); validateTestEvidence(evidence, request);
  if (digest(evidence) !== request.testEvidenceDigest) fail('request', 'test evidence digest mismatch');
  if (bundle) {
    validateBundle(bundle);
    if (bundle.sourceSha !== request.sourceSha || (request.bundleDigest && digest(bundle) !== request.bundleDigest)) fail('request', 'bundle mismatch');
  }
  return request;
}
function validateReport(report) {
  shape('report', report);
  if (report.prerequisiteDigest !== digest(report.manualPrerequisites)) fail('report', 'manual prerequisite digest mismatch');
  unique(report.manualPrerequisites.map(prerequisite => prerequisite.id), 'report.manualPrerequisites');
  if (report.verdict === 'ROUTINE' && (report.uncertainties.length || !report.baseline.bundleDigest || !report.baseline.sourceSha)) fail('report', 'uncertain/unknown baseline cannot be routine');
  return report;
}
function validateEnvironmentState(state) {
  shape('environmentState', state);
  if (state.state === 'READY' && (state.active || state.holdReason)) fail('environmentState', 'ready state cannot have active work or hold');
  if (state.state !== 'READY' && !state.holdReason) fail('environmentState', 'non-ready state requires hold reason');
  if (state.state === 'IN_PROGRESS' && !state.active) fail('environmentState', 'active execution required');
  return state;
}
function assertDeployApproval(request, report, completion) {
  validateRequest(request); validateReport(report);
  if (request.authorization.scope !== 'DEPLOY' || request.id !== report.requestId || request.target !== report.target || request.bundleDigest !== report.candidateBundleDigest || report.verdict !== 'ROUTINE') fail('approval', 'request/report mismatch or maintenance required');
  if (report.manualPrerequisites.length && (!completion || completion.target !== report.target || completion.reportDigest !== digest(report) || completion.prerequisiteDigest !== report.prerequisiteDigest || completion.confirmed !== true)) fail('approval', 'exact manual prerequisite completion required');
}
function summarize(request, bundle, report) {
  const lines = [`Source: ${request.sourceSha}`, `Bundle: ${bundle ? digest(bundle) : 'awaiting approved build'}`, `Target: ${request.target}`, `Tests: all required exact-SHA jobs passed`, `Approval: ${request.authorization.environment}`, 'DMS: routine delivery cannot stop/start/modify capture'];
  if (report) lines.push(`Verdict: ${report.verdict}`, `Pending SQL: see migration evidence ${report.migrationLedgerDigest}`, ...report.manualPrerequisites.flatMap(item => [`BLOCKING OPENSEARCH: ${item.targetHost} / ${item.index} / ${item.alias}`, ...item.steps, ...item.verification]));
  return lines.join('\n');
}
module.exports = {canonicalize,digest,bytesDigest,validateTestEvidence,validateBundle,validateRequest,assertRequestBinding,validateReport,validateEnvironmentState,assertDeployApproval,summarize};
