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

# Install unified CloudWatch-Agent
apt-get install -y wget
wget https://amazoncloudwatch-agent.s3.amazonaws.com/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
dpkg -i -E ./amazon-cloudwatch-agent.deb || true  # dpkg may complain about dependencies
apt-get install -f -y

# Create unified CloudWatch-Agent config
cat >/opt/aws/amazon-cloudwatch-agent.json <<EOF
{
  "logs": {
    "logs_collected": {
      "files": {
        "collect_list": [
          {
            "file_path": "/var/log/nightly-enrichment.log",
            "log_group_name": "/aws/nightly-enrichment/${STAGE_NAME}",
            "log_stream_name": "{instance_id}",
            "timestamp_format": "%Y-%m-%dT%H:%M:%S.%f%z",
            "timezone": "UTC",
            "multi_line_start_pattern": "^\\{"
          }
        ]
      }
    }
  }
}
EOF

# Start CloudWatch Agent
/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl -a fetch-config -m ec2 -c file:/opt/aws/amazon-cloudwatch-agent.json -s

# Ensure log file exists before running the binary
LOG_PATH="/var/log/nightly-enrichment.log"
touch "$LOG_PATH"
chmod 666 "$LOG_PATH"

# Download and install binary
aws s3 cp "s3://${RESOURCE_BUCKET}/nightly-enrichment-${STAGE_NAME}-${COMMIT_SHA}" /usr/local/bin/nightly-enrichment --region eu-central-1
chmod +x /usr/local/bin/nightly-enrichment

# Run binary
echo "[$(date)] Running nightly-enrichment..." | tee -a "$LOG_PATH"
/usr/local/bin/nightly-enrichment >> "$LOG_PATH" 2>&1
EXIT_CODE=$?
echo "[$(date)] Job finished with code $EXIT_CODE" | tee -a "$LOG_PATH"
exit $EXIT_CODE