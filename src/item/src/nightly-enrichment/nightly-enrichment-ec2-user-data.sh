#!/bin/bash
set -euxo pipefail
echo "Running user-data for enrichment for stage $STAGE_NAME at commit $COMMIT_SHA..."

# Wait for network to be ready
until ping -c1 s3.amazonaws.com >/dev/null 2>&1; do
  echo "Waiting for network..."
  sleep 3
done

# Trap ensures termination even on failure
trap 'echo "Instance shutting down..."; \
      INSTANCE_ID=$(curl -s http://169.254.169.254/latest/meta-data/instance-id); \
      aws ec2 terminate-instances --instance-ids "$INSTANCE_ID" --region eu-central-1' EXIT

# Download and install binary
aws s3 cp "s3://${RESOURCE_BUCKET}/nightly-enrichment-${STAGE_NAME}-${COMMIT_SHA}" /usr/local/bin/nightly-enrichment --region eu-central-1
chmod +x /usr/local/bin/nightly-enrichment

# Run binary
echo "[$(date)] Running nightly-enrichment..."
/usr/local/bin/nightly-enrichment
EXIT_CODE=$?
echo "[$(date)] Job finished with code $EXIT_CODE"

exit $EXIT_CODE