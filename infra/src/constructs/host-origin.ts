import type * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import type { Construct } from "constructs";
import type { StageConfig } from "../config";
import { credentialSecrets, credentialValue } from "../credentials";

/**
 * The HTTPS-only custom origin on the host. CloudFront forwards the viewer `Host` through the
 * AllViewer policy, so the host serves the public hostname's certificate and routes by it; the
 * origin DNS name only locates the host. The host refuses a request without the secret header
 * (docs/infra.md#cloudfront-origin-lock).
 */
export function hostOrigin(scope: Construct, config: StageConfig, id: string): cloudfront.CfnDistribution.OriginProperty {
  return {
    id,
    domainName: config.hostOrigin.domainName,
    originCustomHeaders: [{
      headerName: config.hostOrigin.verifyHeaderName,
      headerValue: credentialValue(scope, credentialSecrets(config.stage).originVerify, "value").unsafeUnwrap(),
    }],
    customOriginConfig: {
      httpPort: 80,
      httpsPort: 443,
      originKeepaliveTimeout: 5,
      originProtocolPolicy: "https-only",
      originReadTimeout: 30,
      originSslProtocols: ["TLSv1.2"],
    },
  };
}
