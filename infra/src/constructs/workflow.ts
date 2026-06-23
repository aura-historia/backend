import * as cdk from "aws-cdk-lib";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as sfn from "aws-cdk-lib/aws-stepfunctions";
import * as tasks from "aws-cdk-lib/aws-stepfunctions-tasks";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

export interface PartnerShopApplicationWorkflowProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly worker: lambda.Function;
}

export class PartnerShopApplicationWorkflow extends Construct {
  readonly stateMachine: sfn.StateMachine;

  constructor(scope: Construct, id: string, props: PartnerShopApplicationWorkflowProps) {
    super(scope, id);

    const waitForReview = new tasks.LambdaInvoke(this, "WaitForReview", {
      lambdaFunction: props.worker,
      integrationPattern: sfn.IntegrationPattern.WAIT_FOR_TASK_TOKEN,
      payload: sfn.TaskInput.fromObject({
        step: "WAIT_FOR_REVIEW",
        task_token: sfn.JsonPath.taskToken,
        "partner_application_id.$": "$.partner_application_id",
        "applicant_user_id.$": "$.applicant_user_id",
      }),
    });

    const approve = new tasks.LambdaInvoke(this, "Approve", {
      lambdaFunction: props.worker,
      payload: sfn.TaskInput.fromObject({
        step: "APPROVE",
        "partner_application_id.$": "$.partner_application_id",
        "applicant_user_id.$": "$.applicant_user_id",
      }),
    });

    const reject = new tasks.LambdaInvoke(this, "Reject", {
      lambdaFunction: props.worker,
      payload: sfn.TaskInput.fromObject({
        step: "REJECT",
        "partner_application_id.$": "$.partner_application_id",
        "applicant_user_id.$": "$.applicant_user_id",
      }),
    });

    const decision = new sfn.Choice(this, "ReviewDecision")
      .when(sfn.Condition.stringEquals("$.decision", "APPROVED"), approve)
      .when(sfn.Condition.stringEquals("$.decision", "REJECTED"), reject);

    this.stateMachine = new sfn.StateMachine(this, "PartnerShopApplicationStateMachine", {
      stateMachineName: `partner-shop-application-${props.stageName}`,
      stateMachineType: sfn.StateMachineType.STANDARD,
      definitionBody: sfn.DefinitionBody.fromChainable(waitForReview.next(decision)),
      removalPolicy: props.config.removalPolicy,
    });
  }
}
