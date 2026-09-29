#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

if [[ $# != 1 ]]; then
  fail 'Usage: bash ci/invoke-operation.sh migrate|initialize'
fi
operation="$1"
case "$operation" in
  migrate|initialize) ;;
  *) fail 'Operation must be migrate or initialize.' ;;
esac
case "${STAGE:-}" in
  dev|prod) ;;
  *) fail 'STAGE must be dev or prod.' ;;
esac
if ! [[ "${DEPLOY_COMMIT_SHA:-}" =~ ^[0-9a-f]{40}$ ]]; then
  fail 'DEPLOY_COMMIT_SHA must be a full 40-character lowercase Git commit SHA.'
fi
if [[ -z "${AWS_REGION:-}" ]]; then
  fail 'AWS_REGION must be set to the deployed stage region.'
fi
command -v aws >/dev/null || fail 'AWS CLI is required.'
command -v node >/dev/null || fail 'Node.js is required for strict JSON validation.'

export AWS_PAGER='' AWS_CLI_AUTO_PROMPT=off
umask 077
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

# JSON.parse rejects empty/multiple documents and non-JSON values such as NaN
# (which jq would coerce to null). Never print parse errors or provider content.
json_matches() {
  node - "$@" <<'NODE'
const { readFileSync } = require('node:fs');
const [check, file, expected] = process.argv.slice(2);
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
try {
  const value = JSON.parse(readFileSync(file, 'utf8'));
  let valid = false;
  switch (check) {
    case 'stack':
      valid = object(value) && Array.isArray(value.Stacks) && value.Stacks.length === 1
        && object(value.Stacks[0])
        && ['CREATE_COMPLETE', 'UPDATE_COMPLETE', 'UPDATE_ROLLBACK_COMPLETE'].includes(value.Stacks[0].StackStatus);
      break;
    case 'sha': {
      const parameters = value.Stacks[0].Parameters;
      const commits = Array.isArray(parameters)
        ? parameters.filter((parameter) => object(parameter) && parameter.ParameterKey === 'CommitSHA') : [];
      valid = commits.length === 1 && commits[0].ParameterValue === expected;
      break;
    }
    case 'invocation':
      valid = object(value) && value.StatusCode === 200
        && (!Object.hasOwn(value, 'FunctionError') || value.FunctionError === null);
      break;
    case 'migrate':
      valid = object(value) && Object.keys(value).length === 1 && value.status === 'ready';
      break;
    case 'initialize':
      valid = value === null;
      break;
  }
  process.exitCode = valid ? 0 : 1;
} catch {
  process.exitCode = 1;
}
NODE
}

stack_name="application-${STAGE}-initialize"
printf 'Checking deployed initialization stack for %s in %s at %s.\n' "$operation" "$STAGE" "$DEPLOY_COMMIT_SHA"
if ! aws cloudformation describe-stacks --stack-name "$stack_name" \
  --region "$AWS_REGION" --output json > "$work_dir/stack.json" 2>/dev/null; then
  fail "Cannot read ${stack_name}; confirm it is deployed in the selected account/region and the role allows cloudformation:DescribeStacks."
fi
if ! json_matches stack "$work_dir/stack.json"; then
  fail "Initialization stack ${stack_name} must be CREATE_COMPLETE, UPDATE_COMPLETE or UPDATE_ROLLBACK_COMPLETE; inspect CloudFormation and wait for completion or repair the stack before retrying."
fi
if ! json_matches sha "$work_dir/stack.json" "$DEPLOY_COMMIT_SHA"; then
  fail 'Initialization stack CommitSHA is missing, invalid or does not match DEPLOY_COMMIT_SHA; inspect the deployed release and select its SHA before retrying. No operation was invoked.'
fi

if [[ "$operation" == migrate ]]; then
  function_name="database-migration-lambda-${STAGE}"
  printf '{}\n' > "$work_dir/payload.json"
else
  function_name="fxrate-lambda-${STAGE}"
  # A retry, including one after a new release, must reuse the first-capture ID.
  node - <<'NODE' > "$work_dir/payload.json"
process.stdout.write(JSON.stringify({
  version: '0',
  id: `deployment:fxrate:initial:${process.env.STAGE}:v1`,
  'detail-type': 'Scheduled Event',
  source: 'aura-historia.deployment',
  account: '000000000000',
  time: '1970-01-01T00:00:00Z',
  region: process.env.AWS_REGION,
  resources: [],
  detail: {},
}));
NODE
fi

printf 'Waiting for %s to finish updating.\n' "$function_name"
if ! aws lambda wait function-updated --function-name "$function_name" \
  --region "$AWS_REGION" --cli-connect-timeout 10 --cli-read-timeout 30 >/dev/null 2>&1; then
  fail "Lambda update wait failed or timed out for ${function_name}; inspect LastUpdateStatus and confirm lambda:GetFunctionConfiguration access before retrying. No operation was invoked."
fi

printf 'Invoking %s synchronously.\n' "$function_name"
if ! aws lambda invoke --function-name "$function_name" \
  --region "$AWS_REGION" --invocation-type RequestResponse \
  --payload "fileb://${work_dir}/payload.json" --cli-binary-format raw-in-base64-out \
  --cli-connect-timeout 10 --cli-read-timeout 900 \
  --output json "$work_dir/result.json" > "$work_dir/invocation.json" 2>/dev/null; then
  fail "Synchronous invocation failed for ${function_name}; confirm lambda:InvokeFunction access and inspect restricted Lambda logs. Completion is unknown; check the outcome before retrying."
fi
if ! json_matches invocation "$work_dir/invocation.json"; then
  fail "Invocation of ${function_name} did not confirm StatusCode 200 without FunctionError; inspect restricted Lambda logs before retrying."
fi
if ! json_matches "$operation" "$work_dir/result.json"; then
  if [[ "$operation" == migrate ]]; then
    fail 'Migration did not return exactly {"status":"ready"}; inspect restricted Lambda logs before retrying.'
  fi
  fail 'Initial FX capture did not return exactly JSON null; inspect restricted Lambda logs before retrying with the same stable event ID.'
fi

printf 'Completed %s for %s at %s.\n' "$operation" "$STAGE" "$DEPLOY_COMMIT_SHA"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  printf -- '- Operation: %s\n- Stage: %s\n- Deployed initialization SHA: %s\n' \
    "$operation" "$STAGE" "$DEPLOY_COMMIT_SHA" >> "$GITHUB_STEP_SUMMARY"
fi
