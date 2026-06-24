import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import { ARTIFACT_BUCKET_NAME, MAIL_TEMPLATE_BUCKET_NAME, type StageName } from "./config";
import { stageConfig } from "./config";
import { applicationParameters } from "./parameters";
import { BackendHttpApi } from "./constructs/api";
import { Identity } from "./constructs/cognito";
import { Eventing } from "./constructs/eventing";
import { addUserPoolEnvironment, grantCognitoAdminAccess, Lambdas } from "./constructs/lambdas";
import { Observability } from "./constructs/observability";
import { Search } from "./constructs/opensearch";
import { Queues } from "./constructs/queues";
import { Storage } from "./constructs/storage";
import { PartnerShopApplicationWorkflow } from "./constructs/workflow";

export interface ApplicationStackProps extends cdk.StackProps {
  readonly stage: StageName;
  readonly localStackMappedPort?: string;
}

export class ApplicationStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, {
      ...props,
      synthesizer: new cdk.DefaultStackSynthesizer({
        generateBootstrapVersionRule: false,
      }),
    });

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const parameters = applicationParameters(this);
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia application stack";

    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    const mailTemplateBucket = s3.Bucket.fromBucketName(this, "MailTemplateBucketImport", MAIL_TEMPLATE_BUCKET_NAME);

    const storage = new Storage(this, "Storage", {
      config,
      stageName,
    });

    const queues = new Queues(this, "Queues", {
      config,
      stageName,
    });

    const search = new Search(this, "Search", {
      config,
    });

    const lambdas = new Lambdas(this, "Lambdas", {
      config,
      parameters,
      artifactBucket,
      mailTemplateBucket,
      table: storage.table,
      queues: queues.catalog,
      search,
    });

    const identity = new Identity(this, "Identity", {
      config,
      stageName,
      postConfirmationLambda: lambdas.functions.postConfirmation,
    });
    addUserPoolEnvironment(lambdas.functions, identity.userPool.userPoolId, identity.publicClient.userPoolClientId);
    grantCognitoAdminAccess(lambdas.functions, identity.userPool.userPoolArn);

    const workflow = new PartnerShopApplicationWorkflow(this, "PartnerShopApplicationWorkflow", {
      config,
      stageName,
      worker: lambdas.functions.partnerShopApplicationWorkflow,
    });
    lambdas.functions.partnerShopApplicationApi.addEnvironment(
      "STATE_MACHINE_ARN",
      workflow.stateMachine.stateMachineArn,
    );
    lambdas.functions.partnerShopApplicationApi.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ["states:StartExecution", "states:DescribeExecution"],
        resources: [workflow.stateMachine.stateMachineArn],
      }),
    );
    lambdas.functions.partnerShopApplicationApi.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ["states:SendTaskSuccess", "states:SendTaskFailure", "states:SendTaskHeartbeat"],
        resources: ["*"],
      }),
    );

    const eventing = new Eventing(this, "Eventing", {
      config,
      table: storage.table,
      queues: queues.catalog,
      functions: lambdas.functions,
    });

    const api = new BackendHttpApi(this, "HttpApi", {
      config,
      stageName,
      functions: lambdas.functions,
      identity,
    });

    const observability = new Observability(this, "Observability", {
      config,
      stageName,
      api: api.api,
      table: storage.table,
      functions: lambdas.functions,
    });

    outputs(this, {
      api,
      identity,
      search,
      storage,
      queues,
      eventing,
      observability,
    });
  }
}

function outputs(
  stack: cdk.Stack,
  resources: {
    readonly api: BackendHttpApi;
    readonly identity: Identity;
    readonly search: Search;
    readonly storage: Storage;
    readonly queues: Queues;
    readonly eventing: Eventing;
    readonly observability: Observability;
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
  new cdk.CfnOutput(stack, "ApiGatewayEndpointUrl", { value: resources.api.endpointUrl });
  new cdk.CfnOutput(stack, "DynamodbTable1Name", { value: resources.storage.table.tableName });
  new cdk.CfnOutput(stack, "OpensearchDomainName", { value: resources.search.domainName });
  new cdk.CfnOutput(stack, "OutputOpensearchEndpointUrl", {
    key: "OpensearchEndpointUrl",
    value: resources.search.endpointUrl,
  });

  new cdk.CfnOutput(stack, "NotificationSendQueueUrl", {
    value: resources.queues.catalog.notificationSend.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "NotificationSendDeadLetterQueueUrl", {
    value: resources.queues.catalog.notificationSend.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductMaterializeOpensearchQueueUrl", {
    value: resources.queues.catalog.productMaterializeOpenSearch.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductMaterializeOpensearchDeadLetterQueueUrl", {
    value: resources.queues.catalog.productMaterializeOpenSearch.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductPartnerIngestQueueUrl", {
    value: resources.queues.catalog.productPartnerIngest.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductPartnerIngestDeadLetterQueueUrl", {
    value: resources.queues.catalog.productPartnerIngest.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductPipelineEmbedTextQueueUrl", {
    value: resources.queues.catalog.productPipelineEmbedText.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductPipelineEmbedTextDeadLetterQueueUrl", {
    value: resources.queues.catalog.productPipelineEmbedText.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductPipelineTranslateQueueUrl", {
    value: resources.queues.catalog.productPipelineTranslate.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductPipelineTranslateDeadLetterQueueUrl", {
    value: resources.queues.catalog.productPipelineTranslate.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductUpdateNotifyUserQueueUrl", {
    value: resources.queues.catalog.productUpdateNotifyUser.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ProductUpdateNotifyUserDeadLetterQueueUrl", {
    value: resources.queues.catalog.productUpdateNotifyUser.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "SearchFilterOpenSearchSyncQueueUrl", {
    value: resources.queues.catalog.searchFilterOpenSearchSync.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "SearchFilterOpenSearchSyncDeadLetterQueueUrl", {
    value: resources.queues.catalog.searchFilterOpenSearchSync.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ShopOpensearchIndexQueueUrl", {
    value: resources.queues.catalog.shopOpenSearchIndex.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ShopOpensearchIndexDeadLetterQueueUrl", {
    value: resources.queues.catalog.shopOpenSearchIndex.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ShopifyLambdaQueueUrl", {
    value: resources.queues.catalog.shopify.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ShopifyLambdaDeadLetterQueueUrl", {
    value: resources.queues.catalog.shopify.deadLetterQueue.queueUrl,
  });
  new cdk.CfnOutput(stack, "UserOpensearchIndexQueueUrl", {
    value: resources.queues.catalog.userOpenSearchIndex.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "UserOpensearchIndexDeadLetterQueueUrl", {
    value: resources.queues.catalog.userOpenSearchIndex.deadLetterQueue.queueUrl,
  });

  new cdk.CfnOutput(stack, "OutputStripeEventBusName", {
    key: "StripeEventBusName",
    value: resources.eventing.stripeEventBus.eventBusName,
  });
  new cdk.CfnOutput(stack, "OutputShopifyEventBusName", {
    key: "ShopifyEventBusName",
    value: resources.eventing.shopifyEventBus.eventBusName,
  });

  if (resources.observability.alarmTopic) {
    new cdk.CfnOutput(stack, "AlarmNotificationTopicArn", {
      description: "SNS Topic ARN for CloudWatch alarm notifications",
      value: resources.observability.alarmTopic.topicArn,
    });
  }
}
