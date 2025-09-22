#!/usr/bin/env bash
# script to be called from repository root dir
set -euo pipefail

ITEMS_INDEX_NAME="items"
ITEMS_MAPPING_FILE="opensearch/mappings/items.json"
SHOPS_INDEX_NAME="shops"
SHOPS_MAPPING_FILE="opensearch/mappings/shops.json"

# Resolve OpenSearch domain name + endpoint from CloudFormation Outputs
DOMAIN_NAME=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpensearchDomainName'].OutputValue" \
  --output text)

RAW_ENDPOINT=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpensearchDomainEndpointUrl'].OutputValue" \
  --output text)

if [ -z "$DOMAIN_NAME" ]; then
  echo "❌ Could not resolve OpenSearch domain-name from stack: $STACK_NAME"
  exit 1
fi

if [ -z "$RAW_ENDPOINT" ]; then
  echo "❌ Could not resolve OpenSearch endpoint from stack: $STACK_NAME"
  exit 1
fi

# Strip protocol if included
ENDPOINT=${RAW_ENDPOINT#https://}
echo "✅ Using OpenSearch endpoint: $ENDPOINT"

# Wait until the domain is ACTIVE
echo "⏳ Waiting for OpenSearch domain $DOMAIN_NAME to become ACTIVE..."

while true; do
  PROCESSING=$(aws opensearch describe-domain --domain-name "$DOMAIN_NAME" \
    --query "DomainStatus.Processing" --output text)

  if [ "$PROCESSING" == "False" ]; then
    echo "✅ Domain $DOMAIN_NAME is ACTIVE."
    break
  else
    echo "⏳ Domain still processing... waiting 15s"
    sleep 15
  fi
done

# Configure CLI profile
echo -e "\n es" | opensearch-cli profile create --name "ci" \
  --endpoint "$RAW_ENDPOINT" \
  --auth-type "aws-iam"

# Create indices
echo "📦 Creating index $ITEMS_INDEX_NAME with mapping from $ITEMS_MAPPING_FILE..."
opensearch-cli curl put \
  --path "$ITEMS_INDEX_NAME" \
  --data "@$ITEMS_MAPPING_FILE" \
  --profile ci
echo "📦 Creating index SHOPS_INDEX_NAME with mapping from $SHOPS_MAPPING_FILE..."
opensearch-cli curl put \
  --path "$SHOPS_INDEX_NAME" \
  --data "@$SHOPS_MAPPING_FILE" \
  --profile ci

# Configure refresh-interval for index
if [ "$STAGE" = "prod" ]; then
    echo "Configuring refresh-interval for index $ITEMS_INDEX_NAME..."
    opensearch-cli curl put \
      --path "$ITEMS_INDEX_NAME/_settings" \
      --data '{
        "index": {
          "refresh_interval": "5m"
        }
      }' \
      --profile ci
fi
