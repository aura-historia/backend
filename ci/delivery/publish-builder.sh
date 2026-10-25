#!/usr/bin/env bash
# Operator-only image publication; ordinary releases never rebuild this image.
set -euo pipefail
cd "$(dirname "$0")/../.."
repository=aura-historia-release-builder
region=eu-central-1
account="$(aws sts get-caller-identity --query Account --output text)"
registry="$account.dkr.ecr.$region.amazonaws.com"
tag="tools-$(sha256sum ci/delivery/builder/Dockerfile | cut -d' ' -f1)"
uri="$registry/$repository:$tag"
image_digest="$(aws ecr list-images --region "$region" --repository-name "$repository" --filter tagStatus=TAGGED --query "imageIds[?imageTag=='$tag'].imageDigest | [0]" --output text)"
if [ "$image_digest" != None ]; then
  printf '%s/%s@%s\n' "$registry" "$repository" "$image_digest"
  exit 0
fi
aws ecr get-login-password --region "$region" | docker login --username AWS --password-stdin "$registry"
docker build --platform linux/amd64 --file ci/delivery/builder/Dockerfile --tag "$uri" ci/delivery/builder
docker push "$uri"
image_digest="$(aws ecr describe-images --region "$region" --repository-name "$repository" --image-ids "imageTag=$tag" --query 'imageDetails[0].imageDigest' --output text)"
printf '%s/%s@%s\n' "$registry" "$repository" "$image_digest"
