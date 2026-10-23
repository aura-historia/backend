'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const c = require('./contracts.cjs'), f = require('./fixtures.cjs');
test('canonical hashes and valid immutable contracts round trip', () => {
  assert.equal(c.digest({b:2,a:1}),c.digest({a:1,b:2}));
  c.validateBundle(JSON.parse(JSON.stringify(f.bundle())));
  c.assertRequestBinding(f.request(),f.tests(),f.bundle());
  c.assertDeployApproval(f.request(),f.report());
  for (const value of [NaN,undefined,{value:undefined}]) assert.throws(()=>c.digest(value));
});
test('mixed source, unknown version, incomplete catalogs and failed test evidence fail closed', () => {
  for (const mutate of [b=>b.version=2,b=>b.tests.sourceSha='d'.repeat(40),b=>b.artifacts.shift(),b=>b.images.push({id:'missing',repository:'image',digest:`sha256:${f.hash}`}),b=>b.tests.jobResults.api='skipped',b=>b.artifacts[0].versionId='null']) {
    const value = f.bundle(); mutate(value); assert.throws(()=>c.validateBundle(value));
  }
  for (const field of ['sourceSha','runId','runAttempt']) assert.throws(()=>c.validateTestEvidence(f.tests(),{[field]:field==='runAttempt'?2:'wrong'}));
});
test('bundle identity is reusable while approvals remain target specific', () => {
  const bundle = f.bundle(), before = c.digest(bundle), request = f.request('DEPLOY','dev');
  assert.equal(c.assertRequestBinding(request,f.tests(),bundle),request);
  assert.equal(c.digest(bundle),before);
  request.authorization.environment='aws-prod';
  assert.throws(()=>c.validateRequest(request));
  assert.throws(()=>c.assertDeployApproval(f.request('DEPLOY','dev'),f.report()));
});
test('uncertainty and missing exact manual completion block routine admission', () => {
  const report = f.report(); report.uncertainties=['Missing private mapping'];
  assert.throws(()=>c.validateReport(report));
  report.verdict='MAINTENANCE_REQUIRED'; c.validateReport(report);
  assert.throws(()=>c.assertDeployApproval(f.request(),report));
});
test('failed app deployment preserves independently observed SQL and verified baseline', () => {
  const state={version:1,target:'prod',revision:2,state:'NEEDS_ATTENTION',lastVerified:{bundleDigest:f.hash,sourceSha:f.sha,verifiedAt:f.time},observedSchema:{sha256:f.hash,observedAt:f.time,migrations:[{version:'2',checksum:'e'.repeat(96)}]},active:{executionId:'pipeline-1',requestDigest:f.hash,bundleDigest:'c'.repeat(64)},holdReason:'SQL succeeded; app update failed'};
  c.validateEnvironmentState(state);
  assert.equal(state.observedSchema.migrations[0].version,'2');
  assert.equal(state.lastVerified.bundleDigest,f.hash);
  state.state='READY'; assert.throws(()=>c.validateEnvironmentState(state));
});
