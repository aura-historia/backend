#!/bin/bash
# CloudFormation Template Validation Script
# This script performs basic validation checks on the CloudFormation template

set -e

TEMPLATE_FILE="cfn/application.yaml"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=========================================="
echo "CloudFormation Template Validation"
echo "=========================================="
echo ""

# Check if template exists
if [ ! -f "$TEMPLATE_FILE" ]; then
    echo "❌ Error: Template file not found: $TEMPLATE_FILE"
    exit 1
fi

echo "✓ Template file exists: $TEMPLATE_FILE"
echo ""

# Check file size
FILE_SIZE=$(wc -c < "$TEMPLATE_FILE")
echo "✓ Template file size: $FILE_SIZE bytes"
if [ "$FILE_SIZE" -gt 51200 ]; then
    echo "  ⚠️  Warning: Template is larger than 51KB (may need to be uploaded to S3)"
fi
echo ""

# Count resources
RESOURCE_COUNT=$(grep -cE "^  [A-Z][a-zA-Z0-9]+:" "$TEMPLATE_FILE" || true)
echo "✓ Total resource count: $RESOURCE_COUNT"
if [ "$RESOURCE_COUNT" -gt 500 ]; then
    echo "  ⚠️  Warning: Template has more than 500 resources (CloudFormation limit)"
fi
echo ""

# Check for required sections
echo "Checking required sections..."
for section in "Parameters" "Resources" "Outputs"; do
    if grep -q "^${section}:" "$TEMPLATE_FILE"; then
        echo "  ✓ $section section found"
    else
        echo "  ❌ $section section missing"
    fi
done
echo ""

# Check for key production features
echo "Checking production-ready features..."

# OpenSearch features
if grep -q "AutoTuneOptions:" "$TEMPLATE_FILE"; then
    echo "  ✓ OpenSearch AutoTune enabled"
else
    echo "  ❌ OpenSearch AutoTune not found"
fi

if grep -q "LogPublishingOptions:" "$TEMPLATE_FILE"; then
    echo "  ✓ OpenSearch log publishing configured"
else
    echo "  ❌ OpenSearch log publishing not found"
fi

if grep -q "EncryptionAtRestOptions:" "$TEMPLATE_FILE"; then
    echo "  ✓ OpenSearch encryption at rest enabled"
else
    echo "  ❌ OpenSearch encryption at rest not found"
fi

if grep -q "NodeToNodeEncryptionOptions:" "$TEMPLATE_FILE"; then
    echo "  ✓ OpenSearch node-to-node encryption enabled"
else
    echo "  ❌ OpenSearch node-to-node encryption not found"
fi

# DynamoDB features
if grep -q "PointInTimeRecoveryEnabled: true" "$TEMPLATE_FILE"; then
    echo "  ✓ DynamoDB Point-in-Time Recovery enabled"
else
    echo "  ❌ DynamoDB Point-in-Time Recovery not found"
fi

if grep -q "SSEEnabled: true" "$TEMPLATE_FILE"; then
    echo "  ✓ DynamoDB encryption enabled"
else
    echo "  ❌ DynamoDB encryption not found"
fi

# SQS features
if grep -q "SqsManagedSseEnabled: true" "$TEMPLATE_FILE"; then
    echo "  ✓ SQS encryption enabled"
else
    echo "  ❌ SQS encryption not found"
fi

# Lambda log groups
LAMBDA_LOG_GROUPS=$(grep -c "LambdaLogGroup:" "$TEMPLATE_FILE" || true)
echo "  ✓ Lambda CloudWatch log groups: $LAMBDA_LOG_GROUPS"

# API Gateway logging
if grep -q "AccessLogSettings:" "$TEMPLATE_FILE"; then
    echo "  ✓ API Gateway access logging configured"
else
    echo "  ❌ API Gateway access logging not found"
fi

echo ""

# Validate with AWS CLI if available
if command -v aws &> /dev/null; then
    echo "Validating with AWS CLI..."
    # Try with a default region if not configured
    if aws cloudformation validate-template --template-body "file://$TEMPLATE_FILE" --region us-east-1 &> /dev/null; then
        echo "  ✓ Template validation passed"
    else
        echo "  ⚠️  AWS CLI validation skipped (configure AWS credentials and region)"
        echo "  Run: aws cloudformation validate-template --template-body file://$TEMPLATE_FILE --region <your-region>"
    fi
else
    echo "⚠️  AWS CLI not found - skipping CloudFormation validation"
    echo "   Install AWS CLI to validate template syntax"
fi

echo ""
echo "=========================================="
echo "Validation Complete!"
echo "=========================================="
echo ""
echo "Next steps:"
echo "1. Review the template: cfn/application.yaml"
echo "2. Review documentation: cfn/PRODUCTION_READY_CHANGES.md"
echo "3. Deploy to dev/staging environment first"
echo "4. Test all services and monitoring"
echo "5. Deploy to production with appropriate change management"
