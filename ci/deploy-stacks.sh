#!/usr/bin/env bash
set -euo pipefail

[[ "$STAGE" = dev || "$STAGE" = prod ]]
[[ "$DEPLOY_COMMIT_SHA" =~ ^[0-9a-f]{40}$ ]]
case "$DEPLOY_SCOPE" in foundation|application|all) ;; *) echo 'Use Release for routine deployments. Exceptional scope must be foundation or all.' >&2; exit 1 ;; esac
STACK_NAME_PREFIX="application-${STAGE}"
node ci/release.cjs assert-phase "${DEPLOY_PHASE:-exception}"

# A routine release deploys one recorded stack operation from its prepared assembly.
# Exceptional bootstrap/rollback retains the grouped foundation/all entry points.
deploy() {
  node ci/release.cjs assert-phase "${DEPLOY_PHASE:-exception}"
  npm --prefix infra run cdk -- deploy "$@" \
    --app "$RELEASE_ASSEMBLY" \
    --require-approval never --method change-set --exclusively --previous-parameters \
    --no-path-metadata --no-asset-metadata --no-version-reporting --no-notices
}

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT
compute_parameters=()
if [ "$DEPLOY_SCOPE" != foundation ] && { [ "${DEPLOY_PHASE:-compute}" = compute ] || [ "$DEPLOY_SCOPE" = all ]; }; then
  if ! aws cloudformation describe-stacks --stack-name "${STACK_NAME_PREFIX}-compute" \
    --output json > "$work_dir/compute.json" 2> "$work_dir/error.txt"; then
    if grep -Eq '\(ValidationError\).*does not exist' "$work_dir/error.txt"; then
      : > "$work_dir/compute.json"
    else
      echo 'Cannot inspect compute stack; no deployment attempted.' >&2; exit 1
    fi
  fi
  printf '%s\n' "$CONTAINER_IMAGE_DIGESTS" > "$work_dir/digests.json"
  node ci/container-images.cjs deploy-parameters --stack-file "$work_dir/compute.json" \
    --commit-sha "$DEPLOY_COMMIT_SHA" --digests "$work_dir/digests.json" > "$work_dir/update.json"
  while IFS= read -r assignment; do
    compute_parameters+=(--parameters "${STACK_NAME_PREFIX}-compute:${assignment}")
  done < <(jq -r '.parameters | to_entries[] | "\(.key)=\(.value)"' "$work_dir/update.json")
fi

deploy_foundation() {
  if [[ -z "${DEPLOY_PHASE:-}" || "$DEPLOY_PHASE" = network ]]; then
    bash infra/scripts/ensure-dms-vpc-role.sh
    deploy "${STACK_NAME_PREFIX}-network"
  fi
  if [[ -z "${DEPLOY_PHASE:-}" || "$DEPLOY_PHASE" = data ]]; then deploy "${STACK_NAME_PREFIX}-data"; fi
  if [[ -z "${DEPLOY_PHASE:-}" || "$DEPLOY_PHASE" = initialize ]]; then
    deploy "${STACK_NAME_PREFIX}-initialize" --parameters "${STACK_NAME_PREFIX}-initialize:CommitSHA=${DEPLOY_COMMIT_SHA}"
  fi
}

deploy_application() {
  if [[ -z "${DEPLOY_PHASE:-}" || "$DEPLOY_PHASE" = compute ]]; then deploy "${STACK_NAME_PREFIX}-compute" "${compute_parameters[@]}"; fi
  if [[ -z "${DEPLOY_PHASE:-}" || "$DEPLOY_PHASE" = api ]]; then deploy "${STACK_NAME_PREFIX}-api"; fi
  if [[ "$STAGE" = prod && ( -z "${DEPLOY_PHASE:-}" || "$DEPLOY_PHASE" = observability ) ]]; then deploy "${STACK_NAME_PREFIX}-observability"; fi
}

if [ "$DEPLOY_SCOPE" != application ]; then deploy_foundation; fi
if [ "$DEPLOY_SCOPE" != foundation ]; then deploy_application; fi
