#!/usr/bin/env bash
# script to be called from repository root dir
set -euo pipefail

PRODUCTS_INDEX_NAME="products"
PRODUCTS_MAPPING_FILE="opensearch/mappings/products.json"
SHOPS_INDEX_NAME="shops"
SHOPS_MAPPING_FILE="opensearch/mappings/shops.json"
CATEGORIES_INDEX_NAME="categories"
CATEGORIES_MAPPING_FILE="opensearch/mappings/categories.json"
PERIODS_INDEX_NAME="periods"
PERIODS_MAPPING_FILE="opensearch/mappings/periods.json"

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

# Function to inline synonyms from external files into the mapping JSON
# returning a path to a new temporary file with the inlined synonyms
inline_synonyms() {
  local mapping_file="$1"
  local analysis_dir="./opensearch/analysis"

  local tmp_file
  tmp_file=$(mktemp)

  cp "$mapping_file" "$tmp_file"

  # If mapping has no analysis section, just return it unchanged
  if ! jq -e '.settings.analysis.filter' "$tmp_file" >/dev/null 2>&1; then
    echo "$tmp_file"
    return
  fi

  for syn_path in "$analysis_dir"/*_synonyms.txt; do
    [ -e "$syn_path" ] || continue

    filename=$(basename "$syn_path")
    lang="${filename%%_synonyms.txt}"
    filter_name="${lang}_synonyms"

    if ! jq -e ".settings.analysis.filter[\"$filter_name\"]" "$tmp_file" >/dev/null 2>&1; then
      continue
    fi

    echo "🔄 Inlining synonyms for: $filter_name" >&2

    syn_array=$(grep -v '^\s*#' "$syn_path" \
      | grep -v '^\s*$' \
      | jq -R . \
      | jq -s .)

    tmp2=$(mktemp)

    jq \
      --arg filter "$filter_name" \
      --argjson synonyms "$syn_array" \
      '
      .settings.analysis.filter[$filter]
      |= (del(.synonyms_path, .updateable) + {synonyms: $synonyms})
      ' \
      "$tmp_file" > "$tmp2"

    mv "$tmp2" "$tmp_file"
  done

  echo "$tmp_file"
}

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
    temp_mapping=$(inline_synonyms "$mapping_file")
    opensearch-cli curl put \
      --path "$index_name" \
      --data "@$temp_mapping" \
      --profile ci
    rm "$temp_mapping"
    echo "✅ Index '$index_name' created successfully."
  else
    echo "✅ Index '$index_name' already exists. Skipping creation."
  fi
}

# Create indices
create_index_if_not_exists "$PRODUCTS_INDEX_NAME" "$PRODUCTS_MAPPING_FILE"
create_index_if_not_exists "$SHOPS_INDEX_NAME" "$SHOPS_MAPPING_FILE"
create_index_if_not_exists "$CATEGORIES_INDEX_NAME" "$CATEGORIES_MAPPING_FILE"
create_index_if_not_exists "$PERIODS_INDEX_NAME" "$PERIODS_MAPPING_FILE"
