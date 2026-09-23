import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import {
  ARTIFACT_BUCKET_NAME,
  CLOUDFORMATION_STAGING_BUCKET_NAME,
  MAIL_TEMPLATE_BUCKET_NAME,
  type StageName,
} from "./config";
import { stageConfig } from "./config";
import { applicationParameters, artifactCommitShaParameter } from "./parameters";
import { BackendHttpApi } from "./constructs/api";
import { Identity } from "./constructs/cognito";
import { DmsCdc } from "./constructs/dms-cdc";
import { Eventing } from "./constructs/eventing";
import { Network } from "./constructs/network";
import {
  addUserPoolEnvironment,
  grantCognitoAdminAccess,
  importLambdaCatalog,
  InitializationLambdas,
  Lambdas,
} from "./constructs/lambdas";
import { Observability } from "./constructs/observability";
import { Search } from "./constructs/opensearch";
import { importQueueCatalog, Queues } from "./constructs/queues";
import { Storage } from "./constructs/storage";
import { importWorkerQueueCatalog, WorkerQueues } from "./constructs/worker-queues";

export interface ApplicationStackProps extends cdk.StackProps {
  readonly stage: StageName;
  readonly localStackMappedPort?: string;
}

export interface ApplicationStageProps extends cdk.StackProps {
  readonly stage: StageName;
  readonly stackNamePrefix?: string;
  readonly localStackMappedPort?: string;
}

export interface ApplicationStageStacks {
  readonly network?: ApplicationNetworkStack;
  readonly data: ApplicationDataStack;
  readonly initialization?: ApplicationInitializationStack;
  readonly compute: ApplicationComputeStack;
  readonly api: ApplicationApiStack;
  readonly observability?: ApplicationObservabilityStack;
}

export function createApplicationStacks(scope: Construct, props: ApplicationStageProps): ApplicationStageStacks {
  const stackNamePrefix = props.stackNamePrefix ?? `application-${props.stage}`;
  const baseProps = stackBaseProps(props);
  const network = props.stage === "ephemeral"
    ? undefined
    : new ApplicationNetworkStack(scope, `${stackNamePrefix}-network`, {
        ...baseProps,
        stage: props.stage,
        localStackMappedPort: props.localStackMappedPort,
        stackName: `${stackNamePrefix}-network`,
      });

  const data = new ApplicationDataStack(scope, `${stackNamePrefix}-data`, {
    ...baseProps,
    stage: props.stage,
    localStackMappedPort: props.localStackMappedPort,
    stackName: `${stackNamePrefix}-data`,
    network: network?.network,
  });
  if (network) {
    data.addDependency(network);
  }

  const initialization = props.stage === "ephemeral"
    ? undefined
    : new ApplicationInitializationStack(scope, `${stackNamePrefix}-initialize`, {
        ...baseProps,
        stage: props.stage,
        localStackMappedPort: props.localStackMappedPort,
        stackName: `${stackNamePrefix}-initialize`,
        storage: data.storage,
        network: network?.network,
      });
  initialization?.addDependency(data);
  if (network) {
    initialization?.addDependency(network);
  }

  const compute = new ApplicationComputeStack(scope, `${stackNamePrefix}-compute`, {
    ...baseProps,
    stage: props.stage,
    localStackMappedPort: props.localStackMappedPort,
    stackName: `${stackNamePrefix}-compute`,
    storage: data.storage,
    queues: data.queues,
    search: data.search,
    dmsCdc: data.dmsCdc,
    network: network?.network,
  });
  compute.addDependency(data);
  if (network) {
    compute.addDependency(network);
  }
  if (initialization) {
    compute.addDependency(initialization);
  }

  const api = new ApplicationApiStack(scope, `${stackNamePrefix}-api`, {
    ...baseProps,
    stage: props.stage,
    localStackMappedPort: props.localStackMappedPort,
    stackName: `${stackNamePrefix}-api`,
    identity: compute.identity,
  });
  api.addDependency(compute);

  const observability = props.stage === "prod"
    ? new ApplicationObservabilityStack(scope, `${stackNamePrefix}-observability`, {
        ...baseProps,
        stage: props.stage,
        localStackMappedPort: props.localStackMappedPort,
        stackName: `${stackNamePrefix}-observability`,
        api: api.api,
      })
    : undefined;
  observability?.addDependency(api);
  observability?.addDependency(data);
  observability?.addDependency(compute);

  return {
    network,
    data,
    initialization,
    compute,
    api,
    observability,
  };
}

export class ApplicationNetworkStack extends cdk.Stack {
  readonly network: Network;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    this.templateOptions.description = "Aura Historia private workload network stack";
    this.network = new Network(this, "Network", { config });

    networkOutputs(this, this.network);
  }
}

export interface ApplicationDataStackProps extends ApplicationStackProps {
  readonly network?: Network;
}

export class ApplicationDataStack extends cdk.Stack {
  readonly storage: Storage;
  readonly queues: Queues;
  readonly workerQueues: WorkerQueues;
  readonly search: Search;
  readonly dmsCdc?: DmsCdc;

  constructor(scope: Construct, id: string, props: ApplicationDataStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia data stack";

    this.storage = new Storage(this, "Storage", {
      config,
      network: props.network,
    });

    this.queues = new Queues(this, "Queues", {
      config,
      stageName,
    });
    this.workerQueues = new WorkerQueues(this, "WorkerQueues", { config });

    this.search = new Search(this, "Search", {
      config,
    });
    this.dmsCdc = config.isEphemeral
      ? undefined
      : new DmsCdc(this, "DmsCdc", {
          config,
          network: props.network,
          storage: this.storage,
        });

    dataOutputs(this, {
      storage: this.storage,
      queues: this.queues,
      workerQueues: this.workerQueues,
      search: this.search,
    });
  }
}

export interface ApplicationInitializationStackProps extends ApplicationStackProps {
  readonly storage: Storage;
  readonly network?: Network;
}

export class ApplicationInitializationStack extends cdk.Stack {
  readonly initialization: InitializationLambdas;

  constructor(scope: Construct, id: string, props: ApplicationInitializationStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    if (config.isEphemeral || !props.network || !props.storage.migrationPostgres) {
      throw new Error("Initialization stack requires real PostgreSQL storage and private networking.");
    }

    this.templateOptions.description = "Aura Historia private database initialization stack";
    const commitSha = artifactCommitShaParameter(this);
    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    this.initialization = new InitializationLambdas(this, "InitializationLambdas", {
      config,
      commitSha,
      artifactBucket,
      migrationPostgres: props.storage.migrationPostgres,
      network: props.network,
    });
  }
}

export interface ApplicationComputeStackProps extends ApplicationStackProps {
  readonly storage: Storage;
  readonly queues: Queues;
  readonly search: Search;
  readonly dmsCdc?: DmsCdc;
  readonly network?: Network;
}

export class ApplicationComputeStack extends cdk.Stack {
  readonly lambdas: Lambdas;
  readonly identity: Identity;
  readonly eventing: Eventing;

  constructor(scope: Construct, id: string, props: ApplicationComputeStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const parameters = applicationParameters(this, !config.isEphemeral);
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia compute stack";

    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    const mailTemplateBucket = s3.Bucket.fromBucketName(this, "MailTemplateBucketImport", MAIL_TEMPLATE_BUCKET_NAME);

    this.lambdas = new Lambdas(this, "Lambdas", {
      config,
      parameters,
      artifactBucket,
      mailTemplateBucket,
      postgres: props.storage.postgres,
      search: props.search,
      network: props.network,
    });


    this.identity = new Identity(this, "Identity", {
      config,
      stageName,
      postConfirmationLambda: this.lambdas.functions.postConfirmation,
    });
    addUserPoolEnvironment(
      this.lambdas.functions,
      this.identity.userPool.userPoolId,
      this.identity.publicClient.userPoolClientId,
    );
    grantCognitoAdminAccess(this.lambdas.functions, this.identity.userPool.userPoolArn);

    this.eventing = new Eventing(this, "Eventing", {
      config,
      queues: importQueueCatalog(this, "Queues", stageName),
      workerQueues: importWorkerQueueCatalog(this, "WorkerQueues", config),
      functions: this.lambdas.functions,
      productListingOpenSearchVersion: this.lambdas.productListingOpenSearchVersion,
      productListingNormalizationVersion: this.lambdas.productListingNormalizationVersion,
      searchFilterProjectionVersion: this.lambdas.searchFilterProjectionVersion,
      searchFilterPercolatorVersion: this.lambdas.searchFilterPercolatorVersion,
      searchFilterMatchNotificationVersion: this.lambdas.searchFilterMatchNotificationVersion,
      watchlistNotificationVersion: this.lambdas.watchlistNotificationVersion,
      productListingOpenSearchConsumerActivation: parameters.productListingOpenSearchConsumerActivation,
      productListingNormalizationConsumerActivation: parameters.productListingNormalizationConsumerActivation,
      searchFilterProjectionConsumerActivation: parameters.searchFilterProjectionConsumerActivation,
      searchFilterPercolatorConsumerActivation: parameters.searchFilterPercolatorConsumerActivation,
      searchFilterMatchNotificationConsumerActivation: parameters.searchFilterMatchNotificationConsumerActivation,
      watchlistNotificationConsumerActivation: parameters.watchlistNotificationConsumerActivation,
      cdcRouterActivation: parameters.cdcRouterActivation,
      dmsCdc: props.dmsCdc,
    });

    computeOutputs(this, {
      identity: this.identity,
      eventing: this.eventing,
    });
  }
}

export interface ApplicationApiStackProps extends ApplicationStackProps {
  readonly identity: Identity;
}

export class ApplicationApiStack extends cdk.Stack {
  readonly api: BackendHttpApi;

  constructor(scope: Construct, id: string, props: ApplicationApiStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia API stack";

    this.api = new BackendHttpApi(this, "HttpApi", {
      config,
      stageName,
      functions: importLambdaCatalog(this, "LambdaImports", config),
    });

    new cdk.CfnOutput(this, "ApiGatewayEndpointUrl", { value: this.api.endpointUrl });
    if (this.api.distribution) {
      new cdk.CfnOutput(this, "ApiCloudFrontDistributionDomainName", {
        value: this.api.distribution.attrDomainName,
      });
    }
  }
}

export class ApplicationEphemeralStack extends cdk.Stack {
  readonly storage: Storage;
  readonly queues: Queues;
  readonly workerQueues: WorkerQueues;
  readonly search: Search;
  readonly lambdas: Lambdas;
  readonly identity: Identity;
  readonly eventing: Eventing;
  readonly api: BackendHttpApi;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    if (!config.isEphemeral) {
      throw new Error("ApplicationEphemeralStack only supports the ephemeral stage.");
    }

    const parameters = applicationParameters(this);
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia ephemeral acceptance-test stack";

    this.storage = new Storage(this, "Storage", {
      config,
    });
    this.queues = new Queues(this, "Queues", {
      config,
      stageName,
    });
    this.workerQueues = new WorkerQueues(this, "WorkerQueues", { config });
    this.search = new Search(this, "Search", {
      config,
    });

    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    const mailTemplateBucket = s3.Bucket.fromBucketName(this, "MailTemplateBucketImport", MAIL_TEMPLATE_BUCKET_NAME);

    this.lambdas = new Lambdas(this, "Lambdas", {
      config,
      parameters,
      artifactBucket,
      mailTemplateBucket,
      postgres: this.storage.postgres,
      search: this.search,
    });


    this.identity = new Identity(this, "Identity", {
      config,
      stageName,
      postConfirmationLambda: this.lambdas.functions.postConfirmation,
    });
    addUserPoolEnvironment(
      this.lambdas.functions,
      this.identity.userPool.userPoolId,
      this.identity.publicClient.userPoolClientId,
    );
    grantCognitoAdminAccess(this.lambdas.functions, this.identity.userPool.userPoolArn);

    this.eventing = new Eventing(this, "Eventing", {
      config,
      queues: this.queues.catalog,
      workerQueues: this.workerQueues.catalog,
      functions: this.lambdas.functions,
      productListingOpenSearchVersion: this.lambdas.productListingOpenSearchVersion,
      productListingNormalizationVersion: this.lambdas.productListingNormalizationVersion,
      searchFilterProjectionVersion: this.lambdas.searchFilterProjectionVersion,
      searchFilterPercolatorVersion: this.lambdas.searchFilterPercolatorVersion,
      searchFilterMatchNotificationVersion: this.lambdas.searchFilterMatchNotificationVersion,
      watchlistNotificationVersion: this.lambdas.watchlistNotificationVersion,
      productListingOpenSearchConsumerActivation: parameters.productListingOpenSearchConsumerActivation,
      productListingNormalizationConsumerActivation: parameters.productListingNormalizationConsumerActivation,
      searchFilterProjectionConsumerActivation: parameters.searchFilterProjectionConsumerActivation,
      searchFilterPercolatorConsumerActivation: parameters.searchFilterPercolatorConsumerActivation,
      searchFilterMatchNotificationConsumerActivation: parameters.searchFilterMatchNotificationConsumerActivation,
      watchlistNotificationConsumerActivation: parameters.watchlistNotificationConsumerActivation,
      cdcRouterActivation: parameters.cdcRouterActivation,
    });

    this.api = new BackendHttpApi(this, "HttpApi", {
      config,
      stageName,
      functions: this.lambdas.functions,
    });

    dataOutputs(this, {
      storage: this.storage,
      queues: this.queues,
      workerQueues: this.workerQueues,
      search: this.search,
    });
    computeOutputs(this, {
      identity: this.identity,
      eventing: this.eventing,
    });
    new cdk.CfnOutput(this, "ApiGatewayEndpointUrl", { value: this.api.endpointUrl });
  }
}

export interface ApplicationObservabilityStackProps extends ApplicationStackProps {
  readonly api: BackendHttpApi;
}

export class ApplicationObservabilityStack extends cdk.Stack {
  readonly observability: Observability;

  constructor(scope: Construct, id: string, props: ApplicationObservabilityStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia observability stack";

    this.observability = new Observability(this, "Observability", {
      config,
      stageName,
      api: props.api.api,
      functions: importLambdaCatalog(this, "LambdaAlarmImports", config),
      workerQueues: importWorkerQueueCatalog(this, "WorkerQueueAlarmImports", config),
    });

    if (this.observability.alarmTopic) {
      new cdk.CfnOutput(this, "AlarmNotificationTopicArn", {
        description: "SNS Topic ARN for CloudWatch alarm notifications",
        value: this.observability.alarmTopic.topicArn,
      });
    }
  }
}

function stackBaseProps(props: ApplicationStageProps): cdk.StackProps {
  const { localStackMappedPort: _localStackMappedPort, stackNamePrefix: _stackNamePrefix, stage: _stage, ...stackProps } = props;
  return stackProps;
}

function stackProps(props: ApplicationStackProps): cdk.StackProps {
  return {
    ...stackBaseProps(props),
    synthesizer: new cdk.CliCredentialsStackSynthesizer({
      fileAssetsBucketName: CLOUDFORMATION_STAGING_BUCKET_NAME,
      bucketPrefix: `${props.stage}/`,
    }),
  };
}

function networkOutputs(stack: cdk.Stack, network: Network): void {
  new cdk.CfnOutput(stack, "VpcId", { value: network.vpc.vpcId });
  new cdk.CfnOutput(stack, "NatGatewayEipAllocationId", { value: network.natEip.attrAllocationId });
  new cdk.CfnOutput(stack, "NatGatewayEipPublicIp", { value: network.natEip.attrPublicIp });
  new cdk.CfnOutput(stack, "ApplicationSecurityGroupId", { value: network.applicationSecurityGroup.securityGroupId });
  new cdk.CfnOutput(stack, "DatabaseSecurityGroupId", { value: network.databaseSecurityGroup.securityGroupId });
  new cdk.CfnOutput(stack, "DmsSecurityGroupId", { value: network.dmsSecurityGroup.securityGroupId });
  new cdk.CfnOutput(stack, "DmsEndpointSecurityGroupId", { value: network.dmsEndpointSecurityGroup.securityGroupId });
  new cdk.CfnOutput(stack, "MigrationSecurityGroupId", { value: network.migrationSecurityGroup.securityGroupId });
}

function dataOutputs(
  stack: cdk.Stack,
  resources: {
    readonly search: Search;
    readonly storage: Storage;
    readonly queues: Queues;
    readonly workerQueues: WorkerQueues;
  },
): void {
  new cdk.CfnOutput(stack, "PostgresHost", { value: resources.storage.postgres.host });
  new cdk.CfnOutput(stack, "PostgresPort", { value: resources.storage.postgres.port });
  new cdk.CfnOutput(stack, "PostgresDatabase", { value: resources.storage.postgres.database });
  new cdk.CfnOutput(stack, "OpensearchDomainName", { value: resources.search.domainName });
  new cdk.CfnOutput(stack, "OutputOpensearchEndpointUrl", {
    key: "OpensearchEndpointUrl",
    value: resources.search.endpointUrl,
  });




  new cdk.CfnOutput(stack, "ShopifyLambdaQueueUrl", {
    value: resources.queues.catalog.shopify.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ShopifyLambdaDeadLetterQueueUrl", {
    value: resources.queues.catalog.shopify.deadLetterQueue.queueUrl,
  });
  resources.workerQueues.addOutputs();

}

function computeOutputs(
  stack: cdk.Stack,
  resources: {
    readonly identity: Identity;
    readonly eventing: Eventing;
  },
): void {
  new cdk.CfnOutput(stack, "CognitoHostedUIDomain", {
    value: cdk.Fn.sub("https://${Domain}.auth.${AWS::Region}.amazoncognito.com", {
      Domain: resources.identity.domain.domainName,
    }),
  });
  new cdk.CfnOutput(stack, "CognitoUserPoolId", { value: resources.identity.userPool.userPoolId });
  new cdk.CfnOutput(stack, "CognitoUserPoolClientPublicId", {
    value: resources.identity.publicClient.userPoolClientId,
  });

  new cdk.CfnOutput(stack, "OutputStripeEventBusName", {
    key: "StripeEventBusName",
    value: resources.eventing.stripeEventBus.eventBusName,
  });
  new cdk.CfnOutput(stack, "OutputShopifyEventBusName", {
    key: "ShopifyEventBusName",
    value: resources.eventing.shopifyEventBus.eventBusName,
  });
}
