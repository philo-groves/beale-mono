import { nowIso } from "./ids.js";
import type { ResearchClaimStore } from "./findings.js";
import type { FindingSummary } from "./knowledge-types.js";
import { REPORT_STATUSES, ReportStore, type ReportStatus } from "./reports.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext, ResearchToolExecutionResult } from "./tool-registry.js";
import type { ResearchArtifactRef, ResearchToolAction } from "./types.js";

const LIST_PARAMETERS = { type: "object", properties: { query: { type: "string" }, statuses: { type: "array", items: { type: "string", enum: [...REPORT_STATUSES] } }, limit: { type: "number" } } };
const GET_PARAMETERS = { type: "object", required: ["id"], properties: { id: { type: "string" } } };
const CREATE_PARAMETERS = { type: "object", required: ["title", "summary", "content"], properties: {
  title: { type: "string" }, summary: { type: "string", description: "A concise catalog description." },
  content: { type: "string", description: "The complete Markdown report." }, status: { type: "string", enum: [...REPORT_STATUSES] },
  sourceFindingId: { type: "string", description: "The verified security.chain claim this report documents." },
  sourceChainId: { type: "string", description: "Deprecated legacy alias for sourceFindingId." },
  submissionPacketPath: { type: "string", description: "Path to the completed submission.zip inside the active workspace. Honeycrisp imports it into durable report storage." },
} };
const REVISE_PARAMETERS = { type: "object", required: ["id", "expectedRevision", "content"], properties: {
  id: { type: "string" }, expectedRevision: { type: "number" }, content: { type: "string", description: "The complete replacement Markdown report." },
  summary: { type: "string" }, status: { type: "string", enum: [...REPORT_STATUSES] },
  submissionPacketPath: { type: "string", description: "Optional replacement submission.zip inside the active workspace." },
} };

export interface ReportToolPolicy {
  requireConfirmedChain?: boolean;
  requireSubmissionPacket?: boolean;
  claimStore?: Pick<ResearchClaimStore, "get" | "transition">;
}

export function createReportTools(store: ReportStore, policy: ReportToolPolicy = {}): ResearchExecutableTool[] {
  const requiredCreateFields = [
    ...CREATE_PARAMETERS.required,
    ...(policy.requireConfirmedChain ? ["sourceFindingId"] : []),
    ...(policy.requireSubmissionPacket ? ["submissionPacketPath"] : []),
  ];
  const createParameters = { ...CREATE_PARAMETERS, required: requiredCreateFields };
  return [
    tool("report.list", "report_list", "List workspace reports before creating or revising a shareable result.", "read", LIST_PARAMETERS, (input) => ({ output: store.list({
      ...(text(input.query) ? { query: text(input.query)! } : {}),
      ...(Array.isArray(input.statuses) ? { statuses: strings(input.statuses) as ReportStatus[] } : {}),
      ...(typeof input.limit === "number" ? { limit: input.limit } : {}),
    }) })),
    tool("report.get", "report_get", "Read the full Markdown content of one workspace report.", "read", GET_PARAMETERS, (input) => ({ output: store.get(requiredText(input.id, "id")) })),
    tool("report.create", "report_create", policy.requireConfirmedChain
      ? "Create a complete, revisioned Markdown report for a verified composite finding, attach its durable submission packet, and advance that same claim to report-ready. Leads and isolated unverified findings are not reportable."
      : "Create a complete, revisioned Markdown report when a result is ready to share. Reports are artifacts, not memories.", "write", createParameters, (input, context) => {
      const sourceFinding = policy.requireConfirmedChain
        ? requireReportableSecurityFinding(input.sourceFindingId ?? input.sourceChainId, policy.claimStore)
        : null;
      const created = store.create({
        title: requiredText(input.title, "title"), summary: requiredText(input.summary, "summary"), content: requiredText(input.content, "content"),
        ...(text(input.status) ? { status: text(input.status)! as ReportStatus } : {}),
        ...(text(input.submissionPacketPath) ? { submissionPacketPath: text(input.submissionPacketPath)! } : {}),
      }, context?.modelAuthor);
      const advanced = sourceFinding && policy.claimStore
        ? policy.claimStore.transition(sourceFinding.id, {
            expectedRevision: sourceFinding.revision,
            toStatus: "report_ready",
            reason: `Bound to report ${created.report.id}.`,
            reportId: created.report.id,
            evidence: [{ kind: "report", referenceId: created.report.id, summary: `Complete report ${created.report.title}.` }],
          }, context?.modelAuthor, context?.agentId)
        : null;
      return {
        output: sourceFinding ? { report: created.report, sourceFinding: advanced } : created.report,
        artifactRefs: [created.artifactRef, ...(created.submissionPacketArtifactRef ? [created.submissionPacketArtifactRef] : [])],
      };
    }),
    tool("report.revise", "report_revise", "Replace a report with a complete revised Markdown document, or mark it stale, using its current revision.", "write", REVISE_PARAMETERS, (input, context) => {
      const id = requiredText(input.id, "id");
      const submissionPacketPath = text(input.submissionPacketPath);
      if (policy.requireSubmissionPacket && !submissionPacketPath && !store.get(id)?.submissionPacket) {
        throw new Error("Security reports require an attached submission.zip before revision.");
      }
      const revised = store.revise({
        id, expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"), content: requiredText(input.content, "content"),
        ...(text(input.summary) ? { summary: text(input.summary)! } : {}),
        ...(text(input.status) ? { status: text(input.status)! as ReportStatus } : {}),
        ...(submissionPacketPath ? { submissionPacketPath } : {}),
      }, context?.modelAuthor);
      return { output: revised.report, artifactRefs: [revised.artifactRef, ...(revised.submissionPacketArtifactRef ? [revised.submissionPacketArtifactRef] : [])] };
    }),
  ];
}

function requireReportableSecurityFinding(
  value: unknown,
  claimStore: ReportToolPolicy["claimStore"],
): FindingSummary {
  if (!claimStore) throw new Error("Security report eligibility requires the active claim ledger.");
  const sourceFindingId = requiredText(value, "sourceFindingId");
  const finding = claimStore.get(sourceFindingId);
  if (!finding) throw new Error(`Report source finding is not recorded in this workspace: ${sourceFindingId}.`);
  if (finding.projection !== "finding" || finding.classification !== "security.chain") {
    throw new Error("Security reports require a security.chain composite finding; leads and isolated findings are not report-ready.");
  }
  if (finding.status !== "verified") {
    throw new Error("Security reports require a verified composite finding that has passed independent review.");
  }
  if (!text(finding.impact) || finding.componentClaimIds.length === 0 || finding.evidence.length === 0) {
    throw new Error("Security reports require impact, component findings, and proof evidence on the verified composite finding.");
  }
  for (const componentId of finding.componentClaimIds) {
    const component = claimStore.get(componentId);
    if (!component || component.projection !== "finding" || component.maturity === "refuted") {
      throw new Error(`Security report component is not an active evidence-backed finding: ${componentId}.`);
    }
  }
  return finding;
}

function tool(name: string, transportName: string, description: string, sideEffects: "read" | "write", parameters: Record<string, unknown>, run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => { output: unknown; artifactRefs?: ResearchArtifactRef[] }): ResearchExecutableTool {
  return { descriptor: { name, transportName, description, actionClasses: [sideEffects === "read" ? "recall" : "synthesize"], sideEffects, requiredPermissions: [sideEffects === "read" ? "artifact:read" : "artifact:write"], inputSchema: parameters, metadata: { family: "report", format: "markdown" } }, parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>, async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
    const startedAt = nowIso();
    try { const result = run(isRecord(action.input) ? action.input : {}, context); return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${name} completed.`, output: result.output, ...(result.artifactRefs?.length ? { artifactRefs: result.artifactRefs } : {}), followUpActions: [] }; }
    catch (error) { return { action, status: "error", startedAt, completedAt: nowIso(), summary: `${name} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] }; }
  } };
}
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function text(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function strings(value: unknown): string[] { return Array.isArray(value) ? value.flatMap((item) => text(item) ? [text(item)!] : []) : []; }
function requiredText(value: unknown, field: string): string { if (typeof value !== "string" || !value.trim()) throw new Error(`${field} must be a non-empty string.`); return value.trim(); }
function requiredInteger(value: unknown, field: string): number { if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new Error(`${field} must be an integer.`); return value; }
