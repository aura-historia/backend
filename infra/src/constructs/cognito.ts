import * as cdk from "aws-cdk-lib";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as fs from "node:fs";
import * as path from "node:path";
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
        emailBody: verificationEmailBody(),
      },
      accountRecovery: cognito.AccountRecovery.EMAIL_ONLY,
      removalPolicy: props.config.removalPolicy,
    });

    this.configureUserPool(props);

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
      authFlows: {
        userPassword: true,
        userSrp: true,
      },
      oAuth: {
        flows: { authorizationCodeGrant: true },
        scopes: [cognito.OAuthScope.OPENID, cognito.OAuthScope.EMAIL, cognito.OAuthScope.PROFILE],
        callbackUrls: props.config.cognitoCallbackUrls,
        logoutUrls: props.config.cognitoLogoutUrls,
      },
      readAttributes: new cognito.ClientAttributes().withStandardAttributes({
        email: true,
        emailVerified: true,
      }),
    });

    this.domain = this.userPool.addDomain("PrimaryUserPoolDomain", {
      cognitoDomain: {
        domainPrefix: `primary-userpool-${props.stageName}`,
      },
    });
  }

  private configureUserPool(props: IdentityProps): void {
    const cfnUserPool = this.userPool.node.defaultChild as cognito.CfnUserPool;

    if (props.config.cognitoEmail) {
      cfnUserPool.addPropertyOverride("EmailConfiguration", {
        ConfigurationSet: props.config.cognitoEmail.configurationSet,
        EmailSendingAccount: "DEVELOPER",
        From: props.config.cognitoEmail.from,
        ReplyToEmailAddress: props.config.cognitoEmail.replyTo,
        SourceArn: cdk.Fn.sub("arn:aws:ses:${AWS::Region}:${AWS::AccountId}:identity/${IdentityDomain}", {
          IdentityDomain: props.config.cognitoEmail.identityDomain,
        }),
      });
    }

    if (!props.config.isEphemeral) {
      cfnUserPool.addPropertyOverride("UserPoolAddOns", {
        AdvancedSecurityMode: "ENFORCED",
      });
      cfnUserPool.addPropertyOverride("UserPoolTier", "PLUS");
    }
  }
}

function verificationEmailBody(): string {
  return fs.readFileSync(path.join(__dirname, "..", "templates", "cognito-verification-email.html"), "utf8");
}
