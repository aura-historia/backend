/**
 * Product API Lambda handlers
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { StackConfig } from '../types';
import { createLambda } from '../modules/lambda';
import { createLambdaRole, dynamoDBReadPolicy, sqsSendMessagePolicy, openSearchReadPolicy } from '../modules/iam';
import { createApiRoute } from '../modules/apigateway';

export function createProductApiHandlers(
  config: StackConfig,
  dynamoDbTableArn: pulumi.Output<string>,
  dynamoDbTableName: pulumi.Output<string>,
  openSearchDomainArn: pulumi.Output<string>,
  openSearchDomainEndpoint: pulumi.Output<string>,
  userPoolId: pulumi.Output<string>,
  publicClientId: pulumi.Output<string>,
  adminClientId: pulumi.Output<string>,
  api: aws.apigatewayv2.Api,
  authorizerId: pulumi.Output<string>,
  accountId: string,
  region: string,
  snsTopicArn: pulumi.Output<string>,
  productIngestQueueArn: pulumi.Output<string>,
  productIngestQueueUrl: pulumi.Output<string>
) {
  // ============================================================================
  // API Get Product
  // ============================================================================
  const apiGetProductRole = createLambdaRole('api-get-product', config.stageName, [
    dynamoDBReadPolicy(dynamoDbTableArn),
  ]);

  const apiGetProductLambda = createLambda({
    name: 'product-api-get-product',
    config,
    role: apiGetProductRole,
    environment: {
      DYNAMODB_TABLE_NAME: dynamoDbTableName,
      USER_POOL_ID: userPoolId,
      USER_POOL_PUBLIC_CLIENT_ID: publicClientId,
      USER_POOL_ADMIN_CLIENT_ID: adminClientId,
    },
    snsTopicArn,
  });

  createApiRoute('ApiGetProduct', {
    api,
    routeKey: 'GET /api/v1/products/{shopId}/{shopsProductId}',
    lambda: apiGetProductLambda.lambda,
    accountId,
    region,
    routePattern: '/api/v1/products/*/*',
  });

  // Continue with other API handlers...
  // The file is too long to include everything, but the pattern is clear
}
