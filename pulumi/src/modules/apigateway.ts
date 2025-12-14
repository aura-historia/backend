/**
 * Module for creating API Gateway resources
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface ApiGatewayResources {
  api: aws.apigatewayv2.Api;
  stage: aws.apigatewayv2.Stage;
  authorizer: aws.apigatewayv2.Authorizer;
  api5xxErrorAlarm: aws.cloudwatch.MetricAlarm;
  api4xxErrorAlarm: aws.cloudwatch.MetricAlarm;
}

export function createApiGateway(
  stageName: string,
  userPoolId: pulumi.Output<string>,
  publicClientId: pulumi.Output<string>,
  adminClientId: pulumi.Output<string>,
  region: string,
  snsTopicArn: pulumi.Output<string>
): ApiGatewayResources {
  const api = new aws.apigatewayv2.Api('Api', {
    name: `api-${stageName}`,
    protocolType: 'HTTP',
    corsConfiguration: {
      allowOrigins: ['*'],
      allowMethods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
      allowHeaders: ['*'],
    },
  });

  const stage = new aws.apigatewayv2.Stage('ApiStage', {
    stageName: stageName,
    apiId: api.id,
    autoDeploy: true,
    defaultRouteSettings: {
      throttlingBurstLimit: 50,
      throttlingRateLimit: 20,
    },
  });

  const authorizer = new aws.apigatewayv2.Authorizer('ApiCognitoAuthorizer', {
    apiId: api.id,
    name: 'CognitoJwtAuthorizer',
    authorizerType: 'JWT',
    identitySources: ['$request.header.Authorization'],
    jwtConfiguration: {
      audiences: [publicClientId, adminClientId],
      issuer: pulumi.interpolate`https://cognito-idp.${region}.amazonaws.com/${userPoolId}`,
    },
  });

  const api5xxErrorAlarm = new aws.cloudwatch.MetricAlarm('Api5XXErrorAlarm', {
    alarmName: `${stageName}-api-5xx-errors`,
    alarmDescription: 'Alarm when API Gateway returns 5XX errors',
    metricName: '5XXError',
    namespace: 'AWS/ApiGateway',
    statistic: 'Sum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 5,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      ApiId: api.id,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  const api4xxErrorAlarm = new aws.cloudwatch.MetricAlarm('Api4XXErrorAlarm', {
    alarmName: `${stageName}-api-4xx-errors`,
    alarmDescription: 'Alarm when API Gateway returns high 4XX errors',
    metricName: '4XXError',
    namespace: 'AWS/ApiGateway',
    statistic: 'Sum',
    period: 300,
    evaluationPeriods: 2,
    threshold: 50,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      ApiId: api.id,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  return { api, stage, authorizer, api5xxErrorAlarm, api4xxErrorAlarm };
}

export interface ApiRouteConfig {
  api: aws.apigatewayv2.Api;
  routeKey: string;
  lambda: aws.lambda.Function;
  authorizerId?: pulumi.Output<string>;
  accountId: string;
  region: string;
  routePattern: string;
}

export function createApiRoute(name: string, config: ApiRouteConfig): void {
  const integration = new aws.apigatewayv2.Integration(`${name}Integration`, {
    apiId: config.api.id,
    integrationType: 'AWS_PROXY',
    integrationUri: pulumi.interpolate`arn:aws:lambda:${config.region}:${config.accountId}:function:${config.lambda.name}`,
    payloadFormatVersion: '2.0',
  });

  new aws.apigatewayv2.Route(`${name}Route`, {
    apiId: config.api.id,
    routeKey: config.routeKey,
    target: pulumi.interpolate`integrations/${integration.id}`,
    authorizationType: config.authorizerId ? 'JWT' : undefined,
    authorizerId: config.authorizerId,
  });

  new aws.lambda.Permission(`${name}Permission`, {
    action: 'lambda:InvokeFunction',
    function: config.lambda.name,
    principal: 'apigateway.amazonaws.com',
    sourceArn: pulumi.interpolate`arn:aws:execute-api:${config.region}:${config.accountId}:${config.api.id}/*/*${config.routePattern}`,
  });
}
