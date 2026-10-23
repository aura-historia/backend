'use strict';
const test=require('node:test'), assert=require('node:assert/strict');
const {REQUIRED_JOBS,evidence,assertFullTests}=require('./gate.cjs');
const f=require('./fixtures.cjs');
function input() { return {sourceSha:f.sha,workflowRevision:f.sha,runId:'42',runAttempt:1,needs:Object.fromEntries(REQUIRED_JOBS.map(name=>[name,{result:'success'}])),fingerprints:{toolchainDigest:f.hash,configDigest:f.hash,inventoryDigest:f.hash},completedAt:f.time}; }
test('the trusted full job inventory emits exact request evidence',()=>{
 const result=evidence(input()); assertFullTests(result,{sourceSha:f.sha,runId:'42',runAttempt:1});
 assert.throws(()=>assertFullTests(f.tests()));
 for(const expected of [{sourceSha:'b'.repeat(40)},{runAttempt:2},{runId:'43'}]) assert.throws(()=>assertFullTests(result,expected));
});
test('every failed cancelled skipped or absent required suite prevents eligibility',()=>{
 for(const name of REQUIRED_JOBS) for(const outcome of ['failure','cancelled','skipped',null]) {
  const value=input(); if(outcome)value.needs[name].result=outcome; else delete value.needs[name];
  assert.throws(()=>evidence(value));
 }
});
