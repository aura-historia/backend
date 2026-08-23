const https = require("https");
const { InvokeCommand, LambdaClient } = require("@aws-sdk/client-lambda");

exports.handler = async function (event, context) {
    const physicalResourceId = event.PhysicalResourceId || "fxrate-initial-snapshot";

    try {
        if (event.RequestType === "Create") {
            const props = event.ResourceProperties || {};
            const response = await new LambdaClient({}).send(
                new InvokeCommand({
                    FunctionName: props.FunctionName,
                    InvocationType: "RequestResponse",
                    Payload: Buffer.from(JSON.stringify(eventBridgeEvent(props.SourceEventId))),
                }),
            );
            if (response.StatusCode !== 200 || response.FunctionError) {
                throw new Error(`initial FX snapshot capture failed: ${response.FunctionError || `Lambda status ${response.StatusCode}`}`);
            }
        }

        await respond(event, context, "SUCCESS", {}, physicalResourceId);
    } catch (error) {
        console.error(error);
        await respond(event, context, "FAILED", { Error: error.message || String(error) }, physicalResourceId);
    }
};

function eventBridgeEvent(sourceEventId) {
    return {
        version: "0",
        id: sourceEventId,
        "detail-type": "Scheduled Event",
        source: "aura-historia.deployment",
        account: "000000000000",
        time: "1970-01-01T00:00:00Z",
        region: "eu-central-1",
        resources: [],
        detail: {},
    };
}

exports._test = { eventBridgeEvent };

function respond(event, context, status, data, physicalResourceId) {
    const body = JSON.stringify({
        Status: status,
        Reason: status === "FAILED" ? data.Error : "OK",
        PhysicalResourceId: physicalResourceId || context.logStreamName,
        StackId: event.StackId,
        RequestId: event.RequestId,
        LogicalResourceId: event.LogicalResourceId,
        Data: data,
    });

    return new Promise((resolve, reject) => {
        const url = new URL(event.ResponseURL);
        const request = https.request(
            {
                hostname: url.hostname,
                method: "PUT",
                path: url.pathname + url.search,
                headers: {
                    "content-length": Buffer.byteLength(body),
                    "content-type": "",
                },
            },
            (response) => {
                response.on("data", () => {});
                response.on("end", resolve);
            },
        );
        request.on("error", reject);
        request.write(body);
        request.end();
    });
}
