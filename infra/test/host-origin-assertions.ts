import type { StageName } from "../src/config";
import { credentialSecrets } from "../src/credentials";
import { credentialReference } from "./credential-reference";

/** The HTTPS-only host origin with the secret header; shared by the REST and MCP distributions. */
export function expectHostOrigin(origin: Record<string, unknown>, stage: StageName, id: string): void {
  expect(origin).toEqual({
    Id: id,
    DomainName: `{{resolve:ssm:/origin/${stage}/host-domain-name}}`,
    OriginCustomHeaders: [{
      HeaderName: "X-Origin-Verify",
      HeaderValue: credentialReference(credentialSecrets(stage).originVerify, "value"),
    }],
    CustomOriginConfig: {
      HTTPPort: 80,
      HTTPSPort: 443,
      OriginKeepaliveTimeout: 5,
      OriginProtocolPolicy: "https-only",
      OriginReadTimeout: 30,
      OriginSSLProtocols: ["TLSv1.2"],
    },
  });
}
