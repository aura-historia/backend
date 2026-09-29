#!/usr/bin/env bash
set -euo pipefail
tag="git-${DEPLOY_COMMIT_SHA}"
account="$(aws sts get-caller-identity --query Account --output text)"
uri="${account}.dkr.ecr.${AWS_REGION}.amazonaws.com/${IMAGE_REPOSITORY}"
if ! repository="$(aws ecr describe-repositories --repository-names "$IMAGE_REPOSITORY" --output json)"; then
  echo "Cannot read ECR repository ${IMAGE_REPOSITORY} in ${account}/${AWS_REGION}; if missing, provision it using infra/README.md#container-repository-setup; otherwise inspect the AWS error above." >&2
  exit 1
fi
if ! jq -se --arg account "$account" --arg name "$IMAGE_REPOSITORY" --arg uri "$uri" \
  'length == 1 and (.[0].repositories |
   type == "array" and length == 1 and
   .[0].registryId == $account and
   .[0].repositoryName == $name and
   .[0].repositoryUri == $uri and
   .[0].imageTagMutability == "IMMUTABLE" and
   ((.[0].imageTagMutabilityExclusionFilters // []) == []) and
   .[0].encryptionConfiguration.encryptionType == "AES256")' \
  <<< "$repository" >/dev/null; then
  echo "Invalid ECR repository configuration: expected exactly one repository at ${uri} in account ${account}, IMMUTABLE without exclusions and AES256 encryption. No repository configuration was changed." >&2
  exit 1
fi
aws ecr get-login-password | docker login --username AWS --password-stdin "${uri%/*}"
resolve_tag() {
  node ci/container-images.cjs resolve-one --id "$IMAGE_ID" --commit-sha "$DEPLOY_COMMIT_SHA" "$@"
}
verify_image() {
  local image_ref="$1"
  local inspect_file="$2"
  docker image inspect "$image_ref" > "$inspect_file"
  jq -e --arg sha "$DEPLOY_COMMIT_SHA" \
    --arg source "${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}" \
    --arg binary "$IMAGE_BINARY" \
    --arg workflow_prefix "${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/" \
    --arg architecture "${IMAGE_PLATFORM##*/}" \
    'length == 1 and .[0].Os == "linux" and .[0].Architecture == $architecture and
     .[0].Config.Labels["org.opencontainers.image.revision"] == $sha and
     .[0].Config.Labels["org.opencontainers.image.source"] == $source and
     .[0].Config.Labels["org.opencontainers.image.title"] == $binary and
     (.[0].Config.Labels["com.aura-historia.build-workflow"] | type == "string" and startswith($workflow_prefix) and test("/actions/runs/[0-9]+/attempts/[0-9]+$"))' \
    "$inspect_file" >/dev/null || {
      echo "Image metadata does not match source ${DEPLOY_COMMIT_SHA} and platform ${IMAGE_PLATFORM}." >&2
      exit 1
    }
}
initial="$(resolve_tag --allow-missing)"
if [ "$(jq -r '.status' <<< "$initial")" = existing ]; then
  digest="$(jq -er '.digest' <<< "$initial")"
  docker pull --platform "$IMAGE_PLATFORM" "${uri}@${digest}"
  verify_image "${uri}@${digest}" reused-image.json
  metadata_file=reused-image.json
else
  build_identity="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}/attempts/${GITHUB_RUN_ATTEMPT}"
  local_ref="local/${IMAGE_ID}:${DEPLOY_COMMIT_SHA}-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
  docker build --platform "$IMAGE_PLATFORM" \
    -f "$IMAGE_DOCKERFILE" \
    --build-arg "SOURCE_REVISION=${DEPLOY_COMMIT_SHA}" \
    --build-arg "BUILD_WORKFLOW_IDENTITY=${build_identity}" \
    --label "org.opencontainers.image.source=${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}" \
    --label "org.opencontainers.image.revision=${DEPLOY_COMMIT_SHA}" \
    --label "org.opencontainers.image.title=${IMAGE_BINARY}" \
    --label "com.aura-historia.build-workflow=${build_identity}" \
    -t "$local_ref" .
  verify_image "$local_ref" built-image.json
  bash "ci/container-images/${IMAGE_ID}/smoke.sh" "$local_ref"
  docker tag "$local_ref" "${uri}:${tag}"
  push_succeeded=true
  if ! docker push "${uri}:${tag}" > push.log 2>&1; then
    push_succeeded=false
    cat push.log >&2
    echo "Push failed; checking once for a trusted immutable publication by a competing writer." >&2
  fi
  published="$(resolve_tag --allow-missing)"
  if [ "$(jq -r '.status' <<< "$published")" != existing ]; then
    echo "No trusted registry image is available at ${IMAGE_REPOSITORY}:${tag}; retry publication after inspecting the push failure." >&2
    exit 1
  fi
  digest="$(jq -er '.digest' <<< "$published")"
  docker pull --platform "$IMAGE_PLATFORM" "${uri}@${digest}"
  verify_image "${uri}@${digest}" published-image.json
  if [ "$push_succeeded" = false ]; then
    echo 'Verifying the competing immutable publication with the image-owned smoke test.' >&2
    bash "ci/container-images/${IMAGE_ID}/smoke.sh" "${uri}@${digest}"
  fi
  metadata_file=published-image.json
fi
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo 'Invalid ECR registry digest.' >&2; exit 1; }
build_workflow="$(jq -er '.[0].Config.Labels["com.aura-historia.build-workflow"]' "$metadata_file")"
{
  echo "### Container image: ${IMAGE_ID}"
  echo "- Source SHA: ${DEPLOY_COMMIT_SHA}"
  echo "- Repository: ${IMAGE_REPOSITORY}"
  echo "- Platform: ${IMAGE_PLATFORM}"
  echo "- Registry digest: ${digest}"
  echo "- Build workflow identity: ${build_workflow}"
  echo "- Publisher verification: ${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID} (attempt ${GITHUB_RUN_ATTEMPT})"
} >> "$GITHUB_STEP_SUMMARY"
