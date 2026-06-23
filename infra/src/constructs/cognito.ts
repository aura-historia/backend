import * as cdk from "aws-cdk-lib";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as lambda from "aws-cdk-lib/aws-lambda";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

export interface IdentityProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly postConfirmationLambda: lambda.Function;
}

export class Identity extends Construct {
  readonly userPool: cognito.UserPool;
  readonly publicClient: cognito.UserPoolClient;
  readonly adminClient: cognito.UserPoolClient;
  readonly domain: cognito.UserPoolDomain;

  constructor(scope: Construct, id: string, props: IdentityProps) {
    super(scope, id);

    this.userPool = new cognito.UserPool(this, "PrimaryUserPool", {
      userPoolName: `primary-userpool-${props.stageName}`,
      selfSignUpEnabled: true,
      signInAliases: { email: true },
      autoVerify: { email: true },
      standardAttributes: {
        email: { required: true, mutable: false },
      },
      passwordPolicy: {
        minLength: 8,
        requireLowercase: true,
        requireDigits: true,
        requireSymbols: true,
        requireUppercase: true,
        tempPasswordValidity: cdk.Duration.days(7),
      },
      userVerification: {
        emailSubject: "Verify your email",
        emailBody: "Your verification code is {####}",
      },
      removalPolicy: props.config.removalPolicy,
    });

    this.userPool.addTrigger(cognito.UserPoolOperation.POST_CONFIRMATION, props.postConfirmationLambda);

    this.publicClient = this.userPool.addClient("PrimaryUserPoolClientPublic", {
      userPoolClientName: `primary-userpool-client-public-${props.stageName}`,
      generateSecret: false,
      preventUserExistenceErrors: true,
      enableTokenRevocation: true,
      accessTokenValidity: cdk.Duration.hours(1),
      idTokenValidity: cdk.Duration.hours(1),
      refreshTokenValidity: cdk.Duration.days(30),
      supportedIdentityProviders: [cognito.UserPoolClientIdentityProvider.COGNITO],
      oAuth: {
        flows: { authorizationCodeGrant: true },
        scopes: [cognito.OAuthScope.OPENID, cognito.OAuthScope.EMAIL, cognito.OAuthScope.PROFILE],
        callbackUrls: ["http://localhost:3000"],
        logoutUrls: ["http://localhost:3000"],
      },
      readAttributes: new cognito.ClientAttributes().withStandardAttributes({
        email: true,
        emailVerified: true,
      }),
    });

    this.adminClient = this.userPool.addClient("PrimaryUserPoolClientAdmin", {
      userPoolClientName: `primary-userpool-client-admin-${props.stageName}`,
      generateSecret: false,
      preventUserExistenceErrors: true,
      enableTokenRevocation: true,
      accessTokenValidity: cdk.Duration.hours(1),
      idTokenValidity: cdk.Duration.hours(1),
      refreshTokenValidity: cdk.Duration.days(30),
      authFlows: {
        adminUserPassword: true,
      },
      readAttributes: new cognito.ClientAttributes().withStandardAttributes({
        email: true,
      }),
      writeAttributes: new cognito.ClientAttributes().withStandardAttributes({
        email: true,
      }),
    });

    this.domain = this.userPool.addDomain("PrimaryUserPoolDomain", {
      cognitoDomain: {
        domainPrefix: `primary-userpool-${props.stageName}`,
      },
    });
  }
}
