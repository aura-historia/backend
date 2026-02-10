#!/usr/bin/env bash
# script to be called from repository root dir
set -euo pipefail

PRODUCTS_INDEX_NAME="products"
PRODUCTS_MAPPING_FILE="opensearch/mappings/products.json"
SHOPS_INDEX_NAME="shops"
SHOPS_MAPPING_FILE="opensearch/mappings/shops.json"
CATEGORIES_INDEX_NAME="categories"
CATEGORIES_MAPPING_FILE="opensearch/mappings/categories.json"

# Resolve OpenSearch domain name + endpoint from CloudFormation Outputs
DOMAIN_NAME=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpensearchDomainName'].OutputValue" \
  --output text)

RAW_ENDPOINT=$(aws cloudformation describe-stacks \
  --stack-name "$STACK_NAME" \
  --query "Stacks[0].Outputs[?OutputKey=='OpensearchEndpointUrl'].OutputValue" \
  --output text)

if [ -z "$DOMAIN_NAME" ]; then
  echo "❌ Could not resolve OpenSearch domain-name from stack: '$STACK_NAME'"
  exit 1
fi

if [ -z "$RAW_ENDPOINT" ]; then
  echo "❌ Could not resolve OpenSearch endpoint from stack: '$STACK_NAME'"
  exit 1
fi

# Strip protocol if included
ENDPOINT=${RAW_ENDPOINT#https://}
echo "✅ Using OpenSearch endpoint: '$ENDPOINT'"

# Wait until the domain is ACTIVE
echo "⏳ Waiting for OpenSearch domain '$DOMAIN_NAME' to become ACTIVE..."

while true; do
  PROCESSING=$(aws opensearch describe-domain --domain-name "$DOMAIN_NAME" \
    --query "DomainStatus.Processing" --output text)

  if [ "$PROCESSING" == "False" ]; then
    echo "✅ Domain '$DOMAIN_NAME' is ACTIVE."
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

# Function to create index if it doesn't exist
create_index_if_not_exists() {
  local index_name="$1"
  local mapping_file="$2"

  echo "🔍 Checking if index '$index_name' exists..."

  # Try to get the index; if it exists, the command succeeds and returns index info
  # If it doesn't exist, it returns an error with "index_not_found_exception"
  local response
  response=$(opensearch-cli curl get --path "$index_name" --profile ci 2>&1)

  if echo "$response" | grep -q "index_not_found_exception"; then
    echo "📦 Creating index '$index_name' with mapping from '$mapping_file'..."
    opensearch-cli curl put \
      --path "$index_name" \
      --data "@$mapping_file" \
      --profile ci
    echo "✅ Index '$index_name' created successfully."
  else
    echo "✅ Index '$index_name' already exists. Skipping creation."
  fi
}

# Create indices
create_index_if_not_exists "$PRODUCTS_INDEX_NAME" "$PRODUCTS_MAPPING_FILE"
create_index_if_not_exists "$SHOPS_INDEX_NAME" "$SHOPS_MAPPING_FILE"
create_index_if_not_exists "$CATEGORIES_INDEX_NAME" "$CATEGORIES_MAPPING_FILE"
