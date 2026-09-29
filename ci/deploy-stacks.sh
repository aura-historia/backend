#!/usr/bin/env bash
set -euo pipefail

[[ "$STAGE" = dev || "$STAGE" = prod ]]
[[ "$DEPLOY_COMMIT_SHA" =~ ^[0-9a-f]{40}$ ]]
case "$DEPLOY_SCOPE" in auto|foundation|all) ;; *) echo 'Invalid deployment scope.' >&2; exit 1 ;; esac
STACK_NAME_PREFIX="application-${STAGE}"

deploy() {
  npm --prefix infra run cdk -- deploy "$@" \
    --context "stage=${STAGE}" --context "stackNamePrefix=${STACK_NAME_PREFIX}" \
    --require-approval never --method change-set --exclusively --previous-parameters \
    --no-path-metadata --no-asset-metadata --no-version-reporting --no-notices
}

# Inspect before changing foundation: failed/in-progress stacks require operator recovery.
application_exists=false
stacks=(network data initialize compute api)
if [ "$STAGE" = prod ]; then stacks+=(observability); fi
for stack in "${stacks[@]}"; do
  if aws cloudformation describe-stacks --stack-name "${STACK_NAME_PREFIX}-${stack}" \
    --output json > "${stack}-stack.json" 2>stack-error.txt; then
    status="$(jq -er '.Stacks[0].StackStatus' "${stack}-stack.json")"
    case "$status" in
      CREATE_COMPLETE|UPDATE_COMPLETE|UPDATE_ROLLBACK_COMPLETE) ;;
      *) echo "${stack} is ${status}; repair it before deployment." >&2; exit 1 ;;
    esac
    if [ "$stack" = compute ]; then application_exists=true; fi
  elif grep -Eq '\(ValidationError\).*does not exist' stack-error.txt; then
    : > "${stack}-stack.json"
  else
    cat stack-error.txt >&2
    exit 1
  fi
done

printf '%s\n' "$CONTAINER_IMAGE_DIGESTS" > container-image-digests.json
node ci/container-images.cjs deploy-parameters \
  --stack-file compute-stack.json --commit-sha "$DEPLOY_COMMIT_SHA" \
  --digests container-image-digests.json > compute-update.json
compute_parameters=()
while IFS= read -r assignment; do
  compute_parameters+=(--parameters "${STACK_NAME_PREFIX}-compute:${assignment}")
done < <(jq -r '.parameters | to_entries[] | "\(.key)=\(.value)"' compute-update.json)

migration_changed=false
if [ "$DEPLOY_SCOPE" = auto ] && [ "$application_exists" = true ]; then
  current_sha="$(jq -er '.Stacks[0].Parameters[] | select(.ParameterKey == "CommitSHA") | .ParameterValue' compute-stack.json)"
  [[ "$current_sha" =~ ^[0-9a-f]{40}$ ]]
  # Compare source, not database state. Only an explicit all-scope run acknowledges migration readiness.
  if git --no-pager diff --quiet "$current_sha" "$DEPLOY_COMMIT_SHA" -- migrations infra/sql src/database-migration-lambda; then
    :
  else
    status=$?
    if [ "$status" != 1 ]; then
      echo 'Cannot compare migration sources; no stacks were changed.' >&2
      exit 1
    fi
    migration_changed=true
  fi
fi

bash infra/scripts/ensure-dms-vpc-role.sh
deploy "${STACK_NAME_PREFIX}-network"
deploy "${STACK_NAME_PREFIX}-data"
deploy "${STACK_NAME_PREFIX}-initialize" \
  --parameters "${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"

if [ "$DEPLOY_SCOPE" = foundation ] || [ "$migration_changed" = true ] || { [ "$DEPLOY_SCOPE" = auto ] && [ "$application_exists" = false ]; }; then
  {
    echo '## Foundation deployed — application not deployed'
    echo "- Stage: ${STAGE}; commit: ${DEPLOY_COMMIT_SHA}"
    echo "- Migration sources changed: ${migration_changed}"
    echo '- For forward releases run Migrate at this SHA; never invoke an older migrator during rollback.'
    echo '- On a new environment, also complete OpenSearch setup and Initialize FX.'
    echo '- Then run Deploy for the same ref with scope=all after approving workload activation.'
  } >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi

# scope=all acknowledges schema compatibility and, on first creation, FX/OpenSearch readiness.
# No migration, FX capture, resource import, or workload activation toggle belongs here.
deploy "${STACK_NAME_PREFIX}-compute" "${compute_parameters[@]}"
deploy "${STACK_NAME_PREFIX}-api"
if [ "$STAGE" = prod ]; then deploy "${STACK_NAME_PREFIX}-observability"; fi

if ! aws cloudformation describe-stacks --stack-name "${STACK_NAME_PREFIX}-compute" --output json > compute-stack.json; then
  echo 'Deployment may have completed; inspect CloudFormation before retrying the failed output lookup.' >&2
  exit 1
fi
task_outputs="$(node ci/container-images.cjs task-outputs --stack-file compute-stack.json --stack-name "${STACK_NAME_PREFIX}-compute")"
{
  echo '## Application deployed'
  echo "- Stage: ${STAGE}; commit: ${DEPLOY_COMMIT_SHA}"
  echo "- Image digests: $(jq -cS . container-image-digests.json)"
  echo "$task_outputs"
  echo '- Existing activation parameters preserved. No migrations or initialization were run.'
} >> "$GITHUB_STEP_SUMMARY"
