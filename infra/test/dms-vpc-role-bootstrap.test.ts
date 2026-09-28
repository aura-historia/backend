import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";

const script = resolve(process.cwd(), "scripts", "ensure-dms-vpc-role.sh");
const trust = { Statement: [{ Effect: "Allow", Principal: { Service: "dms.amazonaws.com" }, Action: "sts:AssumeRole" }] };

function runBootstrap(scenario: string): { status: number | null; calls: string[]; stderr: string } {
  const directory = mkdtempSync(join(tmpdir(), "dms-vpc-role-test-"));
  try {
    writeFileSync(join(directory, "aws"), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$AWS_CALLS"
case "$1 $2" in
  'iam get-role')
    if { [ "$DMS_ROLE_SCENARIO" = missing ] || [ "$DMS_ROLE_SCENARIO" = race ]; } && ! grep -q 'iam create-role' "$AWS_CALLS"; then
      echo 'An error occurred (NoSuchEntity) when calling the GetRole operation' >&2
      exit 254
    fi
    if [ "$DMS_ROLE_SCENARIO" = denied ]; then
      echo 'An error occurred (AccessDenied) when calling the GetRole operation' >&2
      exit 254
    fi
    if [ "$DMS_ROLE_SCENARIO" = bad-trust ]; then
      echo '{"Role":{"AssumeRolePolicyDocument":{"Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}}}'
    else
      echo '${JSON.stringify({ Role: { AssumeRolePolicyDocument: trust } })}'
    fi
    ;;
  'iam list-attached-role-policies')
    if [ "$DMS_ROLE_SCENARIO" = ready ]; then
      echo '{"AttachedPolicies":[{"PolicyArn":"arn:aws:iam::aws:policy/service-role/AmazonDMSVPCManagementRole"}]}'
    else
      echo '{"AttachedPolicies":[]}'
    fi
    ;;
  'iam create-role')
    if [ "$DMS_ROLE_SCENARIO" = race ]; then
      echo 'An error occurred (EntityAlreadyExists) when calling the CreateRole operation' >&2
      exit 254
    fi
    ;;
  'iam attach-role-policy'|'iam wait') ;;
  *) echo "Unexpected AWS command: $*" >&2; exit 1 ;;
esac
`);
    writeFileSync(join(directory, "sleep"), "#!/usr/bin/env bash\nexit 0\n");

    chmodSync(join(directory, "aws"), 0o755);
    chmodSync(join(directory, "sleep"), 0o755);
    const callsPath = join(directory, "calls");
    writeFileSync(callsPath, "");
    const result = spawnSync("bash", [script], {
      encoding: "utf8",
      env: { ...process.env, PATH: `${directory}${delimiter}${process.env.PATH}`, AWS_CALLS: callsPath, DMS_ROLE_SCENARIO: scenario },
    });
    return { status: result.status, stderr: result.stderr, calls: readFileSync(callsPath, "utf8").trim().split("\n") };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("account-level DMS role bootstrap", () => {
  test("creates the shared role and attaches the DMS VPC policy if absent", () => {
    const result = runBootstrap("missing");
    expect(result.status).toBe(0);
    expect(result.calls).toEqual(expect.arrayContaining([
      expect.stringContaining("iam create-role --role-name dms-vpc-role"),
      expect.stringContaining("iam wait role-exists --role-name dms-vpc-role"),
      expect.stringContaining("iam attach-role-policy --role-name dms-vpc-role --policy-arn arn:aws:iam::aws:policy/service-role/AmazonDMSVPCManagementRole"),
    ]));
  });

  test("validates a role concurrently created by another stage", () => {
    const result = runBootstrap("race");
    expect(result.status).toBe(0);
    expect(result.calls.some((call) => call.startsWith("iam create-role"))).toBe(true);
    expect(result.calls.some((call) => call.startsWith("iam attach-role-policy"))).toBe(true);
  });

  test("leaves an already configured shared role untouched", () => {
    const result = runBootstrap("ready");
    expect(result.status).toBe(0);
    expect(result.calls).toHaveLength(2);
    expect(result.calls[0]).toContain("iam get-role");
    expect(result.calls[1]).toContain("iam list-attached-role-policies");
  });

  test("attaches the missing managed policy to a correctly trusted existing role", () => {
    const result = runBootstrap("no-policy");
    expect(result.status).toBe(0);
    expect(result.calls.some((call) => call.startsWith("iam create-role"))).toBe(false);
    expect(result.calls.some((call) => call.startsWith("iam attach-role-policy"))).toBe(true);
  });

  test("refuses to change an unexpected trust policy", () => {
    const result = runBootstrap("bad-trust");
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("unexpected trust policy");
    expect(result.calls).toHaveLength(1);
  });

  test("does not treat an IAM access error as a missing role", () => {
    const result = runBootstrap("denied");
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("AccessDenied");
    expect(result.calls).toHaveLength(1);
  });
});
