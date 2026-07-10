const https = require("https");
const {
    CreateWebACLCommand,
    DeleteWebACLCommand,
    GetWebACLCommand,
    ListWebACLsCommand,
    UpdateWebACLCommand,
    WAFV2Client,
} = require("@aws-sdk/client-wafv2");

exports.handler = async function (event, context) {
    let physicalResourceId = event.PhysicalResourceId || "api-cloudfront-web-acl";

    try {
        const props = normalizedProperties(event.ResourceProperties || {});
        let data = {};

        if (event.RequestType === "Delete") {
            await deleteWebAcl(props);
        } else {
            const webAcl = await ensureWebAcl(props);
            physicalResourceId = "web-acl/" + webAcl.Name + "/" + webAcl.Id;
            data = {
                WebAclArn: webAcl.ARN,
                WebAclId: webAcl.Id,
                WebAclName: webAcl.Name,
            };
        }

        await respond(event, context, "SUCCESS", data, physicalResourceId);
    } catch (error) {
        console.error(error);
        await respond(event, context, "FAILED", { Error: error.message || String(error) }, physicalResourceId);
    }
};

function normalizedProperties(props) {
    return {
        description: props.Description || "",
        metricName: props.MetricName,
        name: props.Name,
        region: props.Region || "us-east-1",
        rules: props.Rules || [],
        scope: props.Scope || "CLOUDFRONT",
    };
}

async function ensureWebAcl(props) {
    const client = new WAFV2Client({ region: props.region });
    const existing = await findWebAcl(client, props);
    const input = webAclInput(props);

    if (!existing) {
        const created = await client.send(new CreateWebACLCommand(input));
        return getWebAcl(client, props, created.Summary.Id);
    }

    await client.send(
        new UpdateWebACLCommand({
            ...input,
            Id: existing.Id,
            LockToken: existing.LockToken,
        }),
    );
    return getWebAcl(client, props, existing.Id);
}

async function deleteWebAcl(props) {
    const client = new WAFV2Client({ region: props.region });
    const existing = await findWebAcl(client, props);
    if (!existing) {
        return;
    }

    try {
        await client.send(
            new DeleteWebACLCommand({
                Id: existing.Id,
                LockToken: existing.LockToken,
                Name: props.name,
                Scope: props.scope,
            }),
        );
    } catch (error) {
        if (error.name !== "WAFNonexistentItemException") {
            throw error;
        }
    }
}

async function findWebAcl(client, props) {
    let nextMarker;
    do {
        const response = await client.send(
            new ListWebACLsCommand({
                Limit: 100,
                NextMarker: nextMarker,
                Scope: props.scope,
            }),
        );
        const match = (response.WebACLs || []).find((webAcl) => webAcl.Name === props.name);
        if (match) {
            return getWebAcl(client, props, match.Id);
        }
        nextMarker = response.NextMarker;
    } while (nextMarker);
    return undefined;
}

async function getWebAcl(client, props, id) {
    const response = await client.send(
        new GetWebACLCommand({
            Id: id,
            Name: props.name,
            Scope: props.scope,
        }),
    );
    return { ...response.WebACL, LockToken: response.LockToken };
}

function webAclInput(props) {
    return {
        DefaultAction: { Allow: {} },
        Description: props.description,
        Name: props.name,
        Rules: props.rules,
        Scope: props.scope,
        VisibilityConfig: {
            CloudWatchMetricsEnabled: true,
            MetricName: props.metricName,
            SampledRequestsEnabled: true,
        },
    };
}

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
