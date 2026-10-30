import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { unzipSync } from "fflate";
import { nowIso } from "./ids.js";
import { claimVerificationHash, requireCanonicalClaim, type ResearchClaimStore } from "./findings.js";
import { ReportStore } from "./reports.js";
import { RunbookStore } from "./runbooks.js";
import type { FindingSummary } from "./knowledge-types.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext, ResearchToolExecutionResult } from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

const MAX_UNPACKED_BYTES = 64 * 1024 * 1024;
const MAX_PACKET_FILES = 128;

interface PacketManifest {
  schemaVersion: 1;
  claimId: string;
  replayRunId: string;
  prerequisites: string;
  stepsToReproduce: string;
  expectedResult: string;
  observedResult: string;
  policyRefs: string[];
  files: Array<{ path: string; contentHash: string }>;
}

export function createTriageTools(claims: ResearchClaimStore, runbooks: RunbookStore, reports: ReportStore): ResearchExecutableTool[] {
  const parameters = {
    type: "object",
    required: ["claimId", "expectedRevision", "packetPath", "replayRunId", "reason"],
    properties: {
      claimId: { type: "string", description: "Verified, reportable security finding." },
      expectedRevision: { type: "integer", minimum: 1 },
      packetPath: { type: "string", description: "Workspace ZIP containing README.md, manifest.json, and a poc/ file. The app-server imports it into durable triage storage." },
      replayRunId: { type: "string", description: "Successful full run of the finding's reproduction runbook in a distinct authorized environment." },
      reason: { type: "string", description: "What the independent replay proved and the packet's remaining limits." },
    },
  };
  return [{
    descriptor: { name: "triage.package", transportName: "triage_package",
      description: "Accept a portable PoC ZIP for a verified, reportable finding after a successful full replay in a distinct authorized environment. The packet must include a manifest, reproduction steps, hashes, and a PoC file. Reporting reuses the accepted packet.",
      actionClasses: ["synthesize"], sideEffects: "write", requiredPermissions: ["artifact:write"],
      inputSchema: parameters, metadata: { family: "triage", format: "zip" } },
    parameters,
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try {
        const input = record(action.input);
        const claimId = requiredText(input.claimId, "claimId");
        const replayRunId = requiredText(input.replayRunId, "replayRunId");
        const expectedRevision = requiredInteger(input.expectedRevision, "expectedRevision");
        const finding = claims.get(claimId);
        if (!finding) throw new Error(`Finding not found: ${claimId}.`);
        requireCanonicalClaim(finding);
        if (finding.revision !== expectedRevision) throw new Error("Finding revision changed before triage packaging.");
        if (finding.status !== "verified" || finding.securityTracking?.reportability?.decision !== "reportable") {
          throw new Error("Triage packaging requires a verified finding with a Reportable decision.");
        }
        verifyReplay(finding, runbooks, replayRunId);
        const packetPath = reports.validatedTriagePacketSource(requiredText(input.packetPath, "packetPath"));
        const validatedBytes = readFileSync(packetPath);
        validatePacket(validatedBytes, finding, replayRunId);
        const validatedHash = `sha256:${createHash("sha256").update(validatedBytes).digest("hex")}`;
        const entry = reports.importTriagePacket(claimId, packetPath);
        if (entry.contentHash !== validatedHash) throw new Error("Triage ZIP changed while it was being imported.");
        const updated = claims.recordTriagePacket(claimId, {
          expectedRevision,
          artifactId: entry.id,
          contentHash: entry.contentHash,
          sizeBytes: entry.sizeBytes,
          replayRunId,
          reason: requiredText(input.reason, "reason"),
        }, context?.modelAuthor, context?.agentId);
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: "Verified triage packet accepted.",
          output: { finding: updated, packet: { artifactId: entry.id, contentHash: entry.contentHash, sizeBytes: entry.sizeBytes } },
          followUpActions: [] };
      } catch (error) {
        return { action, status: "error", startedAt, completedAt: nowIso(), summary: "Triage packet rejected.",
          error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  }];
}

function verifyReplay(finding: FindingSummary, runbooks: RunbookStore, runId: string): void {
  if (!finding.reproductionRunbookId || !finding.sourceRevision || !finding.environmentFingerprint) {
    throw new Error("Triage replay requires recorded reproduction, source, and original environment identities.");
  }
  const book = runbooks.get(finding.reproductionRunbookId);
  if (!book) throw new Error("The finding's reproduction runbook is unavailable.");
  const replay = runbooks.getExecution(book.id, runId, { limit: 1 });
  if (replay.status !== "succeeded" || !replay.fullRun || replay.contentRevision !== book.contentRevision
    || replay.sourceRevision !== finding.sourceRevision || !replay.environmentFingerprint
    || replay.environmentFingerprint === finding.environmentFingerprint) {
    throw new Error("Triage requires a successful current full reproduction on a distinct authorized environment.");
  }
}

function validatePacket(bytes: Buffer, finding: FindingSummary, replayRunId: string): PacketManifest {
  if (bytes.length === 0 || bytes.length > 128 * 1024 * 1024) throw new Error("Triage ZIP exceeds the packet size limit.");
  let unpackedBytes = 0;
  let fileCount = 0;
  const files = unzipSync(new Uint8Array(bytes), { filter(info) {
    safeZipPath(info.name);
    if (info.name.endsWith("/")) return false;
    unpackedBytes += info.originalSize;
    fileCount += 1;
    if (fileCount > MAX_PACKET_FILES || unpackedBytes > MAX_UNPACKED_BYTES) {
      throw new Error("Triage ZIP exceeds the unpacked file limit.");
    }
    return true;
  } });
  const names = Object.keys(files);
  if (new Set(names.map((name) => name.toLowerCase())).size !== names.length) {
    throw new Error("Triage ZIP contains file names that collide on a case-insensitive machine.");
  }
  if (!files["README.md"]?.length || !files["manifest.json"]?.length
    || !names.some((name) => name.startsWith("poc/") && name.length > 4)) {
    throw new Error("Triage ZIP requires README.md, manifest.json, and a poc/ file.");
  }
  const manifest: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(files["manifest.json"]));
  if (!isRecord(manifest) || manifest.schemaVersion !== 1 || manifest.claimId !== finding.id
    || manifest.replayRunId !== replayRunId || !Array.isArray(manifest.files)) {
    throw new Error("Triage manifest identity or schema is invalid.");
  }
  const policyRefs = manifest.policyRefs;
  if (!Array.isArray(policyRefs) || !finding.securityTracking?.reportability?.policyRefs.every((ref) => policyRefs.includes(ref))) {
    throw new Error("Triage manifest must include the reportability policy references.");
  }
  for (const key of ["prerequisites", "stepsToReproduce", "expectedResult", "observedResult"] as const) {
    requiredText(manifest[key], `manifest.${key}`);
  }
  const listed = new Set<string>();
  for (const item of manifest.files) {
    if (!isRecord(item)) throw new Error("Triage manifest file entries must be objects.");
    const path = safeZipPath(requiredText(item.path, "manifest file path"));
    if (path === "manifest.json" || listed.has(path) || !files[path]) throw new Error("Triage manifest has a duplicate or missing file.");
    const hash = `sha256:${createHash("sha256").update(files[path]).digest("hex")}`;
    if (item.contentHash !== hash) throw new Error(`Triage packet hash mismatch for ${path}.`);
    listed.add(path);
  }
  if (names.some((name) => name !== "manifest.json" && !listed.has(name))) {
    throw new Error("Triage ZIP contains a file not listed in its manifest.");
  }
  return manifest as unknown as PacketManifest;
}

function safeZipPath(value: string): string {
  const withoutDirectorySuffix = value.endsWith("/") ? value.slice(0, -1) : value;
  if (!withoutDirectorySuffix || value.length > 240 || value.startsWith("/") || value.includes("\\")
    || /[:\u0000-\u001f\u007f]/u.test(value)
    || withoutDirectorySuffix.split("/").some((segment) => segment === ".." || segment === "." || segment === "")) {
    throw new Error("Triage ZIP contains an unsafe path.");
  }
  return value;
}
function record(value: unknown): Record<string, unknown> { if (!isRecord(value)) throw new Error("Tool input must be an object."); return value; }
function isRecord(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === "object" && !Array.isArray(value); }
function requiredText(value: unknown, name: string): string { if (typeof value !== "string" || !value.trim()) throw new Error(`${name} must be nonempty.`); return value.trim(); }
function requiredInteger(value: unknown, name: string): number { if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 1) throw new Error(`${name} must be a positive integer.`); return value; }
