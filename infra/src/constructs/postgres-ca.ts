import * as cdk from "aws-cdk-lib";
import * as lambda from "aws-cdk-lib/aws-lambda";
import { Construct } from "constructs";
import { X509Certificate } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";

export const POSTGRES_CA_PATH = "/opt/postgres-ca/root.pem";

export interface PostgresCaProps {
  /**
   * Literal local layer root containing only postgres-ca/root.pem; immutable during synth.
   * Directories 0755/0555, file 0644/0444, public currently-valid CAs only (at most 1 MiB).
   * Use a Lambda-readable synth umask (e.g. 022); CDK creates staged directories with it.
   */
  readonly assetDirectory: string;
}

type Failure = "INVALID_DIRECTORY" | "INVALID_TREE" | "UNSAFE_PERMISSIONS" | "INVALID_SIZE"
  | "INVALID_PEM" | "NOT_CA" | "INVALID_VALIDITY" | "SOURCE_IO" | "ASSET_STAGING";

class PostgresCaError extends Error {
  constructor(category: Failure) {
    // Deliberately omit filesystem/OpenSSL/CDK causes: they can contain operator input.
    super(`PostgresCa: ${category}`);
  }
}

/** Public trust only; no credential delivery, runtime fetch, or Lambda attachment. */
export class PostgresCa extends Construct {
  readonly layer: lambda.LayerVersion;

  constructor(scope: Construct, id: string, props: PostgresCaProps) {
    super(scope, id);
    const directory = validateAsset(props.assetDirectory);
    try {
      this.layer = new lambda.LayerVersion(this, "Layer", {
        code: lambda.Code.fromAsset(directory, { followSymlinks: cdk.SymlinkFollowMode.NEVER }),
        compatibleRuntimes: [lambda.Runtime.PROVIDED_AL2023],
        compatibleArchitectures: [lambda.Architecture.X86_64],
        // Retain replaced versions for code rollback. Rotate trust separately from credentials.
        removalPolicy: cdk.RemovalPolicy.RETAIN,
      });
    } catch {
      throw new PostgresCaError("ASSET_STAGING");
    }
  }
}

const MAX_BYTES = 1024 * 1024;

function validateAsset(input: string): string {
  if (typeof input !== "string" || input.trim().length === 0 || input.includes("\0") || cdk.Token.isUnresolved(input)) {
    throw new PostgresCaError("INVALID_DIRECTORY");
  }
  // CDK copies file modes but creates directories using umask, not source directory modes.
  const umask = process.umask();
  if ((umask & 0o555) !== 0 || (umask & 0o022) !== 0o022) throw new PostgresCaError("UNSAFE_PERMISSIONS");
  try {
    const directory = path.resolve(input);
    // Check ancestors too: a symlinked parent must not bypass the no-symlinks contract.
    for (let current = directory; ; current = path.dirname(current)) {
      if (!fs.lstatSync(current).isDirectory()) throw new PostgresCaError("INVALID_TREE");
      if (current === path.dirname(current)) break;
    }
    const subdirectory = path.join(directory, "postgres-ca");
    validateDirectory(directory, "postgres-ca");
    validateDirectory(subdirectory, "root.pem");
    const file = path.join(subdirectory, "root.pem");
    const stat = fs.lstatSync(file);
    if (!stat.isFile()) throw new PostgresCaError("INVALID_TREE");
    validateFileMode(stat.mode);
    if (stat.size === 0 || stat.size > MAX_BYTES) throw new PostgresCaError("INVALID_SIZE");

    // Bound the read even if the source grows. Never follow a replaced final symlink or block on a FIFO.
    const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    try {
      const opened = fs.fstatSync(fd);
      if (!opened.isFile()) throw new PostgresCaError("INVALID_TREE");
      validateFileMode(opened.mode);
      const bytes = Buffer.alloc(MAX_BYTES + 1);
      let size = 0;
      while (size < bytes.length) {
        const count = fs.readSync(fd, bytes, size, bytes.length - size, null);
        if (count === 0) break;
        size += count;
      }
      if (size === 0 || size > MAX_BYTES) throw new PostgresCaError("INVALID_SIZE");
      validateBundle(bytes.subarray(0, size));
    } finally {
      fs.closeSync(fd);
    }
    return directory;
  } catch (error) {
    if (error instanceof PostgresCaError) throw error;
    throw new PostgresCaError("SOURCE_IO");
  }
}

function validateDirectory(directory: string, onlyEntry: string): void {
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory()) throw new PostgresCaError("INVALID_TREE");
  // All Lambda users must read/traverse; reject group/world writes and special bits.
  if (![0o755, 0o555].includes(stat.mode & 0o7777)) throw new PostgresCaError("UNSAFE_PERMISSIONS");
  const entries = fs.readdirSync(directory);
  if (entries.length !== 1 || entries[0] !== onlyEntry) throw new PostgresCaError("INVALID_TREE");
}

function validateFileMode(mode: number): void {
  if (![0o644, 0o444].includes(mode & 0o7777)) throw new PostgresCaError("UNSAFE_PERMISSIONS");
}

function validateBundle(bytes: Buffer): void {
  const text = bytes.toString("utf8");
  // Only ASCII whitespace and CERTIFICATE blocks; no headers, provider bodies, or private keys.
  const block = /[ \t\r\n]*-----BEGIN CERTIFICATE-----\r?\n([A-Za-z0-9+/=\r\n]+)\r?\n-----END CERTIFICATE-----[ \t\r\n]*/y;
  let offset = 0;
  let count = 0;
  const now = Date.now();
  while (offset < text.length) {
    const match = block.exec(text);
    if (!match) throw new PostgresCaError("INVALID_PEM");
    const base64 = match[1].replace(/[ \t\r\n]/g, "");
    const der = Buffer.from(base64, "base64");
    if (!base64 || der.toString("base64") !== base64) throw new PostgresCaError("INVALID_PEM");
    let certificate: X509Certificate;
    try {
      certificate = new X509Certificate(der);
    } catch {
      throw new PostgresCaError("INVALID_PEM");
    }
    // OpenSSL accepts trailing DER data; require exactly one certificate per block.
    if (!certificate.raw.equals(der)) throw new PostgresCaError("INVALID_PEM");
    if (!certificate.ca) throw new PostgresCaError("NOT_CA");
    const from = certificate.validFromDate.getTime();
    const to = certificate.validToDate.getTime();
    if (!Number.isFinite(from) || !Number.isFinite(to) || from >= to || now < from || now >= to) {
      throw new PostgresCaError("INVALID_VALIDITY");
    }
    offset = block.lastIndex;
    count++;
  }
  if (count === 0) throw new PostgresCaError("INVALID_PEM");
}
