/**
 * Module for creating Cognito User Pools and related resources
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface CognitoResources {
  userPool: aws.cognito.UserPool;
  publicClient: aws.cognito.UserPoolClient;
  adminClient: aws.cognito.UserPoolClient;
  domain: aws.cognito.UserPoolDomain;
}

export function createUserPool(
  stageName: string,
  postConfirmationLambdaArn: pulumi.Output<string>
): CognitoResources {
  const userPool = new aws.cognito.UserPool('PrimaryUserPool', {
    name: `primary-userpool-${stageName}`,
    usernameAttributes: ['email'],
    autoVerifiedAttributes: ['email'],
    passwordPolicy: {
      minimumLength: 8,
      requireUppercase: true,
      requireLowercase: true,
      requireNumbers: true,
      requireSymbols: true,
      temporaryPasswordValidityDays: 7,
    },
    userPoolAddOns: {
      advancedSecurityMode: 'ENFORCED',
    },
    verificationMessageTemplate: {
      defaultEmailOption: 'CONFIRM_WITH_CODE',
      emailSubject: 'Verify your email',
      emailMessage: 'Your verification code is {####}',
    },
    schemas: [
      {
        name: 'email',
        attributeDataType: 'String',
        mutable: false,
        required: true,
      },
    ],
    lambdaConfig: {
      postConfirmation: postConfirmationLambdaArn,
    },
  });

  const publicClient = new aws.cognito.UserPoolClient('PrimaryUserPoolClientPublic', {
    userPoolId: userPool.id,
    name: `primary-userpool-client-public-${stageName}`,
    generateSecret: false,
    preventUserExistenceErrors: 'ENABLED',
    allowedOauthFlowsUserPoolClient: true,
    allowedOauthFlows: ['code'],
    allowedOauthScopes: ['openid', 'email', 'profile'],
    supportedIdentityProviders: ['COGNITO'],
    callbackUrls: ['http://localhost:3000/', 'https://aura-historia.com/'],
    logoutUrls: ['http://localhost:3000/', 'https://aura-historia.com/'],
    enableTokenRevocation: true,
    accessTokenValidity: 1,
    idTokenValidity: 1,
    refreshTokenValidity: 30,
    tokenValidityUnits: {
      accessToken: 'hours',
      idToken: 'hours',
      refreshToken: 'days',
    },
    readAttributes: ['email', 'email_verified'],
  });

  const adminClient = new aws.cognito.UserPoolClient('PrimaryUserPoolClientAdmin', {
    userPoolId: userPool.id,
    name: `primary-userpool-client-admin-${stageName}`,
    generateSecret: false,
    preventUserExistenceErrors: 'ENABLED',
    allowedOauthFlowsUserPoolClient: false,
    explicitAuthFlows: ['ALLOW_ADMIN_USER_PASSWORD_AUTH'],
    enableTokenRevocation: true,
    accessTokenValidity: 1,
    idTokenValidity: 1,
    refreshTokenValidity: 30,
    tokenValidityUnits: {
      accessToken: 'hours',
      idToken: 'hours',
      refreshToken: 'days',
    },
    readAttributes: ['email'],
    writeAttributes: ['email'],
  });

  const domain = new aws.cognito.UserPoolDomain('PrimaryUserPoolDomain', {
    domain: `primary-userpool-${stageName}`,
    userPoolId: userPool.id,
  });

  return { userPool, publicClient, adminClient, domain };
}
