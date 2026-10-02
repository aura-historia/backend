import * as cdk from "aws-cdk-lib";
import * as cognito from "aws-cdk-lib/aws-cognito";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as fs from "node:fs";
import * as path from "node:path";
import { Construct } from "constructs";
import { ssmValue, type CognitoIdentityProviderConfig, type StageConfig } from "../config";

export interface IdentityProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly postConfirmationLambda: lambda.Function;
  readonly preSignUpLambda?: lambda.Function;
}

interface CreatedIdentityProvider {
  readonly resource: Construct;
  readonly clientIdentityProvider: cognito.UserPoolClientIdentityProvider;
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
      // Federated source subjects are derived from usernames, so preserve case.
      signInCaseSensitive: true,
      autoVerify: { email: true },
      standardAttributes: {
        email: { required: true, mutable: true },
        givenName: { required: false, mutable: true },
        familyName: { required: false, mutable: true },
        locale: { required: false, mutable: true },
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

    const identityProviders = createIdentityProviders(this, this.userPool, props.config.cognitoIdentityProviders);

    this.userPool.addTrigger(cognito.UserPoolOperation.POST_CONFIRMATION, props.postConfirmationLambda);
    if (props.config.cognitoIdentityProviders.some((provider) => provider.autoLinkVerifiedEmail)) {
      if (!props.preSignUpLambda) {
        throw new Error("Verified-email identity linking requires the pre-sign-up Lambda.");
      }
      this.userPool.addTrigger(cognito.UserPoolOperation.PRE_SIGN_UP, props.preSignUpLambda);
    }

    this.publicClient = this.userPool.addClient("PrimaryUserPoolClientPublic", {
      userPoolClientName: `primary-userpool-client-public-${props.stageName}`,
      generateSecret: false,
      preventUserExistenceErrors: true,
      enableTokenRevocation: true,
      accessTokenValidity: cdk.Duration.hours(1),
      idTokenValidity: cdk.Duration.hours(1),
      refreshTokenValidity: cdk.Duration.days(30),
      supportedIdentityProviders: [
        cognito.UserPoolClientIdentityProvider.COGNITO,
        ...identityProviders.map((provider) => provider.clientIdentityProvider),
      ],
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
        givenName: true,
        familyName: true,
        locale: true,
      }),
    });
    for (const provider of identityProviders) {
      this.publicClient.node.addDependency(provider.resource);
    }

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

function createIdentityProviders(
  scope: Construct,
  userPool: cognito.UserPool,
  providers: readonly CognitoIdentityProviderConfig[],
): CreatedIdentityProvider[] {
  return providers.map((provider) => createIdentityProvider(scope, userPool, provider));
}

function createIdentityProvider(
  scope: Construct,
  userPool: cognito.UserPool,
  provider: CognitoIdentityProviderConfig,
): CreatedIdentityProvider {
  switch (provider.kind) {
    case "google": {
      const resource = new cognito.UserPoolIdentityProviderGoogle(scope, `${provider.providerName}IdentityProvider`, {
        userPool,
        clientId: ssmValue(provider.clientIdParameterName),
        clientSecretValue: cdk.SecretValue.ssmSecure(provider.clientSecretParameterName),
        scopes: [...provider.scopes],
        attributeMapping: {
          email: cognito.ProviderAttribute.GOOGLE_EMAIL,
          emailVerified: cognito.ProviderAttribute.GOOGLE_EMAIL_VERIFIED,
          givenName: cognito.ProviderAttribute.GOOGLE_GIVEN_NAME,
          familyName: cognito.ProviderAttribute.GOOGLE_FAMILY_NAME,
          locale: cognito.ProviderAttribute.other("locale"),
        },
      });

      return {
        resource,
        clientIdentityProvider: cognito.UserPoolClientIdentityProvider.GOOGLE,
      };
    }
    default:
      throw new Error(`Unsupported Cognito identity provider kind: ${String(provider.kind)}`);
  }
}

function verificationEmailBody(): string {
  return fs.readFileSync(path.join(__dirname, "..", "resources", "cognito-verification-email.html"), "utf8");
}
