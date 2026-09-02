import { createHash } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { nowIso } from "./ids.js";
import type {
  ResearchExecutableTool,
  ResearchToolExecutionResult,
} from "./tool-registry.js";
import type {
  ResearchResourceKind,
  ResearchToolAction,
  ResearchWorkspaceResourceContext,
} from "./types.js";

export type ResearchResourceSource = "explicit_scope" | "runtime_discovery";
export type ResearchResourceReviewStatus = "unreviewed" | "relevant" | "not_relevant" | "explicit_out_of_scope";

export interface TrackedResearchResource {
  id: string;
  workspaceId: string;
  kind: ResearchResourceKind;
  name: string;
  locator: string;
  source: ResearchResourceSource;
  direction: "in_scope" | "out_of_scope" | null;
  scopeAssetId: string | null;
  rationale: string;
  reviewStatus: ResearchResourceReviewStatus;
  reviewReason: string | null;
  discoveredAt: string;
  updatedAt: string;
}

export interface ResearchResourceScopeReviewRequest {
  resource: TrackedResearchResource;
  purpose: string;
  campaignObjective?: string;
  authorizationRecorded: boolean;
}

export interface ResearchResourceScopeReviewDecision {
  decision: "relevant" | "not_relevant";
  reason: string;
  source: "auto_review" | "policy";
  reviewer?: { provider: string; model: string };
  usage?: Record<string, unknown>;
}

export type ResearchResourceScopeAuthorizer = (
  request: ResearchResourceScopeReviewRequest,
  signal?: AbortSignal,
) => Promise<ResearchResourceScopeReviewDecision>;

export interface ResearchResourceCatalogOptions {
  databasePath: string;
  workspaceId: string;
  explicitResources?: readonly ResearchWorkspaceResourceContext[];
}

/**
 * Durable inventory metadata. Discovery rows deliberately do not enter the
 * model-authorship ledger: classifying an ambient dependency is inventory,
 * not authoring target content, a claim, or an authorization grant.
 */
export class ResearchResourceCatalog {
  readonly #database: DatabaseSync;
  readonly #workspaceId: string;

  public constructor(options: ResearchResourceCatalogOptions) {
    this.#database = openResearchDatabase(options.databasePath);
    this.#workspaceId = requiredText(options.workspaceId, "workspaceId");
    this.#migrate();
    this.#replaceExplicitResources(options.explicitResources ?? []);
  }

  public close(): void {
    this.#database.close();
  }

  public list(): TrackedResearchResource[] {
    return (this.#database.prepare(`
      SELECT * FROM app_server_research_resources
      WHERE workspace_id = ? ORDER BY updated_at DESC, id
    `).all(this.#workspaceId) as Record<string, unknown>[]).map(decodeResource);
  }

  public get(id: string): TrackedResearchResource | null {
    const row = this.#database.prepare(`
      SELECT * FROM app_server_research_resources WHERE workspace_id = ? AND id = ?
    `).get(this.#workspaceId, requiredText(id, "resourceId")) as Record<string, unknown> | undefined;
    return row ? decodeResource(row) : null;
  }

  public discover(input: {
    kind: ResearchResourceKind;
    name: string;
    locator: string;
    rationale: string;
  }): TrackedResearchResource {
    const kind = input.kind;
    const name = requiredText(input.name, "name");
    const locator = requiredText(input.locator, "locator");
    const rationale = requiredText(input.rationale, "rationale");
    const existing = this.#findByIdentity(kind, locator);
    if (existing) {
      this.#database.prepare(`
        UPDATE app_server_research_resources
        SET name = ?, rationale = CASE WHEN source = 'explicit_scope' THEN rationale ELSE ? END, updated_at = ?
        WHERE id = ?
      `).run(name, rationale, nowIso(), existing.id);
      return this.get(existing.id)!;
    }
    const timestamp = nowIso();
    const id = stableResourceId(this.#workspaceId, kind, locator);
    this.#database.prepare(`
      INSERT INTO app_server_research_resources (
        id, workspace_id, kind, name, locator, normalized_locator, source,
        direction, scope_asset_id, rationale, review_status, review_reason,
        discovered_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, 'runtime_discovery', NULL, NULL, ?, 'unreviewed', NULL, ?, ?)
    `).run(id, this.#workspaceId, kind, name, locator, normalizeLocator(locator), rationale, timestamp, timestamp);
    return this.get(id)!;
  }

  public hasTouch(resourceId: string, revisionKey: string): boolean {
    return Boolean(this.#database.prepare(`
      SELECT 1 FROM app_server_research_resource_touches
      WHERE resource_id = ? AND revision_key = ?
    `).get(requiredText(resourceId, "resourceId"), requiredText(revisionKey, "revisionKey")));
  }

  public recordReview(resourceId: string, decision: ResearchResourceScopeReviewDecision): TrackedResearchResource {
    const reviewStatus = decision.decision === "relevant" ? "relevant" : "not_relevant";
    this.#database.prepare(`
      UPDATE app_server_research_resources
      SET review_status = ?, review_reason = ?, updated_at = ?
      WHERE workspace_id = ? AND id = ?
    `).run(reviewStatus, boundedText(decision.reason, 2_000), nowIso(), this.#workspaceId, resourceId);
    return this.get(resourceId)!;
  }

  public recordTouch(resourceId: string, revisionKey: string): string {
    const touchedAt = nowIso();
    this.#database.prepare(`
      INSERT OR IGNORE INTO app_server_research_resource_touches(resource_id, revision_key, touched_at)
      VALUES (?, ?, ?)
    `).run(requiredText(resourceId, "resourceId"), requiredText(revisionKey, "revisionKey"), touchedAt);
    return touchedAt;
  }

  #findByIdentity(kind: ResearchResourceKind, locator: string): TrackedResearchResource | null {
    const row = this.#database.prepare(`
      SELECT * FROM app_server_research_resources
      WHERE workspace_id = ? AND kind = ? AND normalized_locator = ?
    `).get(this.#workspaceId, kind, normalizeLocator(locator)) as Record<string, unknown> | undefined;
    return row ? decodeResource(row) : null;
  }

  #upsertExplicit(resource: ResearchWorkspaceResourceContext): void {
    const existing = this.#findByIdentity(resource.kind, resource.locator);
    const timestamp = nowIso();
    const id = existing?.id ?? stableResourceId(this.#workspaceId, resource.kind, resource.locator);
    const reviewStatus = resource.direction === "out_of_scope" ? "explicit_out_of_scope" : existing?.reviewStatus ?? "unreviewed";
    this.#database.prepare(`
      INSERT INTO app_server_research_resources (
        id, workspace_id, kind, name, locator, normalized_locator, source,
        direction, scope_asset_id, rationale, review_status, review_reason,
        discovered_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, 'explicit_scope', ?, ?, ?, ?, NULL, ?, ?)
      ON CONFLICT(id) DO UPDATE SET
        kind = excluded.kind,
        name = excluded.name,
        locator = excluded.locator,
        normalized_locator = excluded.normalized_locator,
        source = 'explicit_scope',
        direction = excluded.direction,
        scope_asset_id = excluded.scope_asset_id,
        rationale = excluded.rationale,
        review_status = CASE
          WHEN excluded.direction = 'out_of_scope' THEN 'explicit_out_of_scope'
          WHEN app_server_research_resources.review_status = 'explicit_out_of_scope' THEN 'unreviewed'
          ELSE app_server_research_resources.review_status
        END,
        updated_at = excluded.updated_at
    `).run(
      id,
      this.#workspaceId,
      resource.kind,
      resource.name ?? resource.locator,
      resource.locator,
      normalizeLocator(resource.locator),
      resource.direction,
      resource.id,
      resource.instruction ?? "Operator-recorded scope resource.",
      reviewStatus,
      timestamp,
      timestamp,
    );
  }

  #replaceExplicitResources(resources: readonly ResearchWorkspaceResourceContext[]): void {
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      this.#database.prepare(`
        UPDATE app_server_research_resources
        SET source = 'runtime_discovery', direction = NULL, scope_asset_id = NULL,
            review_status = 'unreviewed', review_reason = NULL, updated_at = ?
        WHERE workspace_id = ? AND source = 'explicit_scope'
      `).run(nowIso(), this.#workspaceId);
      for (const resource of resources) this.#upsertExplicit(resource);
      this.#database.exec("COMMIT");
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  #migrate(): void {
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS app_server_research_resources (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('domain', 'repository', 'binary', 'service', 'tool', 'documentation', 'other')),
        name TEXT NOT NULL,
        locator TEXT NOT NULL,
        normalized_locator TEXT NOT NULL,
        source TEXT NOT NULL CHECK(source IN ('explicit_scope', 'runtime_discovery')),
        direction TEXT CHECK(direction IS NULL OR direction IN ('in_scope', 'out_of_scope')),
        scope_asset_id TEXT,
        rationale TEXT NOT NULL,
        review_status TEXT NOT NULL CHECK(review_status IN ('unreviewed', 'relevant', 'not_relevant', 'explicit_out_of_scope')),
        review_reason TEXT,
        discovered_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(workspace_id, kind, normalized_locator)
      );
      CREATE INDEX IF NOT EXISTS app_server_research_resources_workspace_updated
        ON app_server_research_resources(workspace_id, updated_at DESC);
      CREATE TABLE IF NOT EXISTS app_server_research_resource_touches (
        resource_id TEXT NOT NULL REFERENCES app_server_research_resources(id) ON DELETE CASCADE,
        revision_key TEXT NOT NULL,
        touched_at TEXT NOT NULL,
        PRIMARY KEY(resource_id, revision_key)
      );
    `);
  }
}

export interface ResearchResourceToolOptions {
  catalog: ResearchResourceCatalog;
  authorizeScopeRelevance: ResearchResourceScopeAuthorizer;
  campaignObjective?: string;
  authorizationRecorded: boolean;
  sourceRevision?: string;
  environmentFingerprint?: string;
}

const RESOURCE_PARAMETERS = {
  type: "object",
  required: ["operation"],
  properties: {
    operation: { type: "string", enum: ["list", "discover", "touch"] },
    kind: { type: "string", enum: ["domain", "repository", "binary", "service", "tool", "documentation", "other"] },
    name: { type: "string" },
    locator: { type: "string", description: "Stable path, service identifier, package/binary name, URL, or other canonical locator." },
    rationale: { type: "string", description: "Why this ambient resource may be relevant to the active campaign." },
    resourceId: { type: "string" },
    purpose: { type: "string", description: "The bounded research purpose for the proposed first touch." },
    revision: { type: "string", description: "Optional resource build, version, commit, or environment identity." },
  },
};

export function createResearchResourceTool(options: ResearchResourceToolOptions): ResearchExecutableTool {
  return {
    descriptor: {
      name: "resource.catalog",
      transportName: "resource_catalog",
      description: "List explicitly scoped and ambient research resources; classify a discovered binary, service, tool, repository, domain, or documentation source without authoring target content; or request Auto-Review before recording its first research touch. Discovery never grants authorization and never triggers bug-history work by itself.",
      actionClasses: ["recall", "inspect"],
      sideEffects: "write",
      requiredPermissions: ["research-resource:inventory"],
      inputSchema: RESOURCE_PARAMETERS,
      metadata: {
        provider: "appServer.built_in",
        safetyProfile: "non-authoring-resource-inventory",
        discoveryAuthorship: "none",
      },
    },
    parameters: RESOURCE_PARAMETERS as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action, context) {
      const operation = requiredEnum(action.input.operation, "operation", ["list", "discover", "touch"] as const);
      if (operation === "list") {
        return complete(action, "Listed tracked research resources.", {
          resources: options.catalog.list(),
          discoveryIsNonAuthoring: true,
        });
      }
      if (operation === "discover") {
        const resource = options.catalog.discover({
          kind: requiredEnum(action.input.kind, "kind", ["domain", "repository", "binary", "service", "tool", "documentation", "other"] as const),
          name: requiredString(action.input.name, "name"),
          locator: requiredString(action.input.locator, "locator"),
          rationale: requiredString(action.input.rationale, "rationale"),
        });
        return complete(action, `Classified ambient ${resource.kind} ${resource.name} without triggering first touch.`, {
          resource,
          discoveryIsNonAuthoring: true,
          authorizationChanged: false,
          firstTouchTriggered: false,
        });
      }
      return touchResource(options, action, context?.signal);
    },
  };
}

async function touchResource(
  options: ResearchResourceToolOptions,
  action: ResearchToolAction,
  signal?: AbortSignal,
): Promise<ResearchToolExecutionResult> {
  const resourceId = requiredString(action.input.resourceId, "resourceId");
  const purpose = requiredString(action.input.purpose, "purpose");
  const outcome = await reviewResearchResourceFirstTouch(options, {
    resourceId,
    purpose,
    ...(typeof action.input.revision === "string" && action.input.revision.trim()
      ? { revision: action.input.revision.trim() }
      : {}),
    ...(signal ? { signal } : {}),
  });
  if (outcome.status === "blocked") {
    return blocked(action, outcome.summary, outcome.output);
  }
  return complete(action, outcome.summary, outcome.output, outcome.followUpActions);
}

export async function reviewResearchResourceFirstTouch(
  options: ResearchResourceToolOptions,
  input: { resourceId: string; purpose: string; revision?: string; signal?: AbortSignal },
): Promise<{
  status: "complete" | "blocked";
  summary: string;
  output: Record<string, unknown>;
  followUpActions: readonly string[];
}> {
  const resourceId = requiredText(input.resourceId, "resourceId");
  const purpose = requiredText(input.purpose, "purpose");
  const resource = options.catalog.get(resourceId);
  if (!resource) return touchOutcome("blocked", `Tracked research resource not found: ${resourceId}`, {});
  if (resource.direction === "out_of_scope") {
    return touchOutcome("blocked", `First touch denied because ${resource.name} is explicitly out of scope.`, { resource });
  }
  const revisionKey = requiredString(
    input.revision ?? options.sourceRevision ?? options.environmentFingerprint ?? "unversioned",
    "revision",
  );
  if (options.catalog.hasTouch(resource.id, revisionKey)) {
    return touchOutcome("complete", `Research resource ${resource.name} was already touched for ${revisionKey}.`, {
      resource,
      revisionKey,
      firstTouch: false,
    });
  }
  let review: ResearchResourceScopeReviewDecision;
  try {
    review = await options.authorizeScopeRelevance({
      resource,
      purpose,
      ...(options.campaignObjective ? { campaignObjective: options.campaignObjective } : {}),
      authorizationRecorded: options.authorizationRecorded,
    }, input.signal);
  } catch (error) {
    return touchOutcome("blocked", `Auto-Review failed closed before resource first touch: ${errorMessage(error)}`, { resource });
  }
  const reviewed = options.catalog.recordReview(resource.id, review);
  if (review.decision !== "relevant") {
    return touchOutcome("blocked", `Auto-Review did not verify campaign relevance for ${resource.name}: ${review.reason}`, {
      resource: reviewed,
      review,
      firstTouch: false,
    });
  }
  const touchedAt = options.catalog.recordTouch(resource.id, revisionKey);
  const reminder = researchHistoryReminder(reviewed.kind);
  return touchOutcome("complete", `Auto-Review verified relevance and recorded first touch of ${resource.name}.`, {
    resource: reviewed,
    revisionKey,
    firstTouch: true,
    touchedAt,
    review,
    authorizationChanged: false,
    reminder,
  }, [
    "Complete the resource-first-touch provenance, advisory, release-note, and source-history baseline before broad exploration.",
  ]);
}

function touchOutcome(
  status: "complete" | "blocked",
  summary: string,
  output: Record<string, unknown>,
  followUpActions: readonly string[] = [],
): { status: "complete" | "blocked"; summary: string; output: Record<string, unknown>; followUpActions: readonly string[] } {
  return { status, summary, output, followUpActions };
}

export function researchHistoryReminder(kind: ResearchResourceKind): readonly string[] {
  return [
    "Record the exact resource identity, build or version, platform image, provenance, and relationship to the active campaign.",
    "Search vendor and ecosystem security advisories, CVEs, security bulletins, and referenced fixes using product, component, service, binary, and symbol aliases.",
    "Review release notes, security-content pages, fixed-version records, and version-to-version changes for disclosed fixes and silent hardening.",
    "Inspect upstream history and vendor forks or source drops. For Apple components include Apple Open Source releases, upstream projects, tags, blame, fix commits, and downstream divergences when available.",
    kind === "binary" || kind === "service" || kind === "tool"
      ? "Map the installed binary, service, or tool to its package, source repository, launch or entitlement context, exposed interfaces, and historical component names before concluding history is absent."
      : "Use repository, package, protocol, and symbol aliases to connect public history to the exact component under review.",
    "Record matches, likely variants, explicit no-match queries with dates, and any deferred source whose absence limits the novelty assessment.",
    "Scope relevance does not itself expand authorization for live targets, accounts, networks, or devices.",
  ];
}

function complete(
  action: ResearchToolAction,
  summary: string,
  output: unknown,
  followUpActions: readonly string[] = [],
): ResearchToolExecutionResult {
  const timestamp = nowIso();
  return { action, status: "complete", startedAt: timestamp, completedAt: timestamp, summary, output, followUpActions };
}

function blocked(action: ResearchToolAction, summary: string, output?: unknown): ResearchToolExecutionResult {
  const timestamp = nowIso();
  return { action, status: "blocked", startedAt: timestamp, completedAt: timestamp, summary, ...(output === undefined ? {} : { output }), followUpActions: [] };
}

function decodeResource(row: Record<string, unknown>): TrackedResearchResource {
  const direction = row.direction === "in_scope" || row.direction === "out_of_scope" ? row.direction : null;
  const source = row.source === "explicit_scope" ? "explicit_scope" : "runtime_discovery";
  const reviewStatus = row.review_status === "relevant" || row.review_status === "not_relevant" || row.review_status === "explicit_out_of_scope"
    ? row.review_status
    : "unreviewed";
  return {
    id: String(row.id),
    workspaceId: String(row.workspace_id),
    kind: row.kind as ResearchResourceKind,
    name: String(row.name),
    locator: String(row.locator),
    source,
    direction,
    scopeAssetId: typeof row.scope_asset_id === "string" ? row.scope_asset_id : null,
    rationale: String(row.rationale),
    reviewStatus,
    reviewReason: typeof row.review_reason === "string" ? row.review_reason : null,
    discoveredAt: String(row.discovered_at),
    updatedAt: String(row.updated_at),
  };
}

function stableResourceId(workspaceId: string, kind: ResearchResourceKind, locator: string): string {
  return `resource_${createHash("sha256").update(`${workspaceId}\n${kind}\n${normalizeLocator(locator)}`).digest("hex").slice(0, 32)}`;
}

function normalizeLocator(value: string): string {
  return value.trim().replace(/\\/gu, "/").replace(/\/+$/u, "").toLocaleLowerCase();
}

function requiredString(value: unknown, name: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${name} is required.`);
  return value.trim();
}

function requiredText(value: string, name: string): string {
  return requiredString(value, name);
}

function requiredEnum<const T extends readonly string[]>(value: unknown, name: string, allowed: T): T[number] {
  const text = requiredString(value, name);
  if (!allowed.includes(text)) throw new Error(`${name} must be one of: ${allowed.join(", ")}.`);
  return text as T[number];
}

function boundedText(value: string, max = 2_000): string {
  const normalized = value.trim().replace(/\s+/gu, " ");
  return normalized.length <= max ? normalized : normalized.slice(0, max);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
