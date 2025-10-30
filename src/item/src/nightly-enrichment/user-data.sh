#!/bin/bash
set -euxo pipefail
echo "Running user-data for enrichment for stage $STAGE_NAME at commit $COMMIT_SHA..."

# Trap ensures termination even on failure
trap 'echo "Instance shutting down..."; \
      INSTANCE_ID=$(curl -s http://169.254.169.254/latest/meta-data/instance-id); \
      aws ec2 terminate-instances --instance-ids "$INSTANCE_ID" --region eu-central-1' EXIT

# Create unified CloudWatch-Agent config
cat << 'EOF' | sudo tee /opt/aws/amazon-cloudwatch-agent.json > /dev/null
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
sudo amazon-cloudwatch-agent-ctl -a fetch-config -m ec2 -c file:/opt/aws/amazon-cloudwatch-agent.json -s

# Ensure log file exists before running the binary
LOG_PATH="/var/log/nightly-enrichment.log"
sudo touch "$LOG_PATH"
sudo chmod 666 "$LOG_PATH"

# Download and install binary
sudo aws s3 cp "s3://${ARTIFACT_BUCKET}/nightly-enrichment-${STAGE_NAME}-${COMMIT_SHA}" /usr/local/bin/nightly-enrichment --region eu-central-1
sudo chmod +x /usr/local/bin/nightly-enrichment

# Prepare (preconfigured via custom ami) python venv
cd /home/ubuntu/
source /opt/enrichment-env/bin/activate
export PATH="/opt/enrichment-env/bin:$PATH"

# Run binary
echo "[$(date)] Running nightly-enrichment..." | tee -a "$LOG_PATH"
/usr/local/bin/nightly-enrichment >> "$LOG_PATH" 2>&1
EXIT_CODE=$?
echo "[$(date)] Job finished with code $EXIT_CODE" | tee -a "$LOG_PATH"
exit $EXIT_CODE