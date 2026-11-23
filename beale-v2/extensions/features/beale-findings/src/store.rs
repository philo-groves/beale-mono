//! Findings persistence in the shared host database.
//!
//! The Findings feature owns these tables and applies [`MIGRATIONS`] under its
//! feature ID; builds without Findings never create them. Evidence checks read
//! the host's saved tool calls.

use anyhow::{Context, Result, ensure};
use beale_contracts::findings::{
    EvidenceDigest, EvidenceRef, Finding, GetResponse, QueryPage, QueryRequest, Revision, Status,
    Summary,
};
use beale_storage::{
    HostDatabase,
    rusqlite::{Connection, OptionalExtension, params},
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Ordered Findings schema versions.
pub const MIGRATIONS: &[&str] = &[V1];

const V1: &str = "
CREATE TABLE finding_counters (
    workspace_id TEXT PRIMARY KEY,
    next_id INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE findings (
    workspace_id TEXT NOT NULL,
    id TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, id)
);
CREATE INDEX findings_workspace_updated ON findings(workspace_id, updated_at_ms DESC, id);
CREATE TABLE finding_revisions (
    workspace_id TEXT NOT NULL,
    id TEXT NOT NULL,
    number INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, id, number)
);
CREATE TABLE finding_tool_sources (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    output_sha256 TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    agent_role TEXT NOT NULL DEFAULT '',
    tool_created_at_ms INTEGER NOT NULL DEFAULT 0
);
";

/// Creates or upgrades the Findings tables.
pub fn migrate(db: &HostDatabase) -> Result<()> {
    db.migrate_schema(crate::FEATURE_ID, MIGRATIONS)
}

/// Tool names whose successful results can support a validation.
pub const EVIDENCE_TOOLS: [&str; 4] = ["read", "bash", "powershell", "findings_verify"];

/// A tool result from the current, not yet committed turn.
pub struct EvidenceSource<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub output: &'a str,
    pub is_error: bool,
    pub agent_role: &'a str,
    pub created_at_ms: i64,
}

/// Constraints tying validation evidence to the finding it supports.
pub struct EvidenceBinding<'a> {
    /// Evidence must have been produced at or after this time.
    pub not_before_ms: i64,
    /// Stored agent role that must have produced the evidence, when required.
    pub agent_role: Option<&'a str>,
}

pub fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .min(i64::MAX as u128) as u64)
}

fn snapshot(finding: &Finding) -> Result<String> {
    let mut compact = finding.clone();
    compact.history.clear();
    Ok(serde_json::to_string(&compact)?)
}

fn insert_revision(
    connection: &Connection,
    workspace_id: &str,
    id: &str,
    revision: &Revision,
) -> Result<()> {
    connection.execute(
        "INSERT INTO finding_revisions(workspace_id,id,number,at_ms,session_id,reason,before_json,after_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![workspace_id,id,revision.number as i64,revision.at_ms as i64,revision.session_id,revision.reason,revision.before_json,revision.after_json],
    )?;
    Ok(())
}

fn validate_evidence(refs: &[EvidenceRef]) -> Result<()> {
    ensure!(refs.len() <= 32, "too many finding evidence references");
    let mut seen = HashSet::new();
    for item in refs {
        ensure!(
            !item.reference.trim().is_empty()
                && item.reference.len() <= 512
                && item.note.as_ref().is_none_or(|note| note.len() <= 1_000)
                && seen.insert((item.kind as u8, item.reference.as_str())),
            "invalid or duplicate finding evidence reference"
        );
    }
    Ok(())
}

fn validate_attack_path_list(items: &[String], limit: usize, item_limit: usize) -> Result<()> {
    ensure!(items.len() <= limit, "too many finding attack path entries");
    ensure!(
        items
            .iter()
            .all(|item| !item.trim().is_empty() && item.len() <= item_limit),
        "invalid finding attack path entry"
    );
    Ok(())
}

fn validate(finding: &Finding) -> Result<()> {
    ensure!(!finding.workspace_id.is_empty(), "workspace is required");
    ensure!(
        !finding.title.trim().is_empty() && finding.title.len() <= 200,
        "invalid finding title"
    );
    ensure!(
        finding.summary.len() <= 4_000,
        "finding summary is too long"
    );
    if let Some(root_cause) = &finding.root_cause {
        ensure!(
            !root_cause.summary.trim().is_empty() && root_cause.summary.len() <= 4_000,
            "invalid finding root cause summary"
        );
        validate_evidence(&root_cause.evidence_refs)?;
    }
    if let Some(path) = &finding.attack_path {
        ensure!(
            !path.summary.trim().is_empty() && path.summary.len() <= 4_000,
            "invalid finding attack path summary"
        );
        ensure!(
            !path.reachability.summary.trim().is_empty()
                && path.reachability.summary.len() <= 4_000,
            "invalid finding reachability summary"
        );
        ensure!(
            path.likelihood_description.len() <= 4_000
                && path.impact_description.len() <= 4_000
                && path.reachability.attacker_role.len() <= 200
                && path.reachability.entrypoint.len() <= 1_000
                && path.reachability.source.len() <= 1_000
                && path.reachability.sink.len() <= 1_000,
            "finding attack path field is too long"
        );
        validate_attack_path_list(&path.assumptions, 32, 1_000)?;
        validate_attack_path_list(&path.limitations, 32, 1_000)?;
        validate_attack_path_list(&path.preconditions, 32, 1_000)?;
        validate_attack_path_list(&path.steps, 64, 2_000)?;
        validate_evidence(&path.evidence_refs)?;
    }
    if let Some(remediation) = &finding.remediation {
        ensure!(
            !remediation.trim().is_empty() && remediation.len() <= 4_000,
            "invalid finding remediation"
        );
    }
    ensure!(finding.surface_ids.len() <= 16, "too many finding surfaces");
    let mut surface_ids = HashSet::new();
    for id in &finding.surface_ids {
        ensure!(
            !id.is_empty() && id.len() <= 32 && surface_ids.insert(id),
            "invalid or duplicate surface ID"
        );
    }
    ensure!(
        !finding.state.candidate.hypothesis.trim().is_empty()
            && finding.state.candidate.hypothesis.len() <= 4_000,
        "invalid candidate hypothesis"
    );
    validate_evidence(
        finding
            .state
            .candidate
            .evidence_refs
            .as_deref()
            .unwrap_or(&[]),
    )?;
    ensure!(
        finding.state.candidate.revision > 0 && finding.state.candidate.completed_at_ms > 0,
        "invalid candidate state"
    );
    if let Some(state) = &finding.state.validated {
        ensure!(state.revision > 0, "invalid validated state");
        validate_evidence(state.evidence_refs.as_deref().unwrap_or(&[]))?;
        validate_evidence(&state.counter_evidence)?;
        for value in [
            &state.attacker_control,
            &state.violated_invariant,
            &state.observed_behavior,
            &state.impact,
            &state.validation_description,
            &state.validation_result,
        ] {
            ensure!(value.len() <= 4_000, "validated field is too long");
        }
        ensure!(
            state.validation_method.len() <= 200,
            "validation method is too long"
        );
        ensure!(
            state.validation_limitations.len() <= 32,
            "too many validation limitations"
        );
        ensure!(
            state
                .validation_limitations
                .iter()
                .all(|item| !item.trim().is_empty() && item.len() <= 1_000),
            "invalid validation limitation"
        );
    }
    if let Some(state) = &finding.state.screened {
        ensure!(state.revision > 0, "invalid screening revision");
        ensure!(
            state.impact_score.is_none_or(|score| score <= 100)
                && state.reachability_score.is_none_or(|score| score <= 100),
            "screening scores must be integers from 0 to 100"
        );
        validate_evidence(state.evidence_refs.as_deref().unwrap_or(&[]))?;
        for check in [
            &state.validation_review,
            &state.known_issue_review,
            &state.scope_review,
        ] {
            ensure!(
                check.method.len() <= 2_000 && check.conclusion.len() <= 2_000,
                "invalid screening check"
            );
        }
    }
    if let Some(state) = &finding.state.reported {
        validate_evidence(state.evidence_refs.as_deref().unwrap_or(&[]))?;
        ensure!(
            state.revision > 0
                && state.poc_steps.len() <= 8_000
                && state.poc_result.len() <= 4_000
                && state.poc_portability_notes.len() <= 4_000
                && state.report_reference.len() <= 512
                && state.poc_artifact_refs.len() <= 32,
            "invalid reporting state"
        );
        ensure!(
            state
                .poc_artifact_refs
                .iter()
                .all(|reference| !reference.is_empty() && reference.len() <= 512),
            "invalid PoC reference"
        );
    }
    if let Some(state) = &finding.state.rejected {
        validate_evidence(state.evidence_refs.as_deref().unwrap_or(&[]))?;
        ensure!(
            !state.reason.trim().is_empty()
                && state.reason.len() <= 2_000
                && state.from != Status::Rejected,
            "invalid rejection"
        );
    }
    Ok(())
}

/// Findings operations on the host database.
pub trait FindingStore {
    /// Saves a current-turn tool result offered as validation evidence before
    /// its turn is committed, keeping its digest, role, and execution time.
    fn finding_record_tool_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        source: &EvidenceSource<'_>,
    ) -> Result<()>;
    fn finding_get(&self, workspace_id: &str, id: &str) -> Result<Option<Finding>>;
    fn finding_get_page(
        &self,
        workspace_id: &str,
        id: &str,
        history_offset: Option<u32>,
    ) -> Result<Option<GetResponse>>;
    fn finding_query(&self, request: &QueryRequest) -> Result<QueryPage>;
    fn finding_create(&self, finding: Finding, session_id: &str) -> Result<Finding>;
    fn finding_update(
        &self,
        finding: Finding,
        expected_revision: u64,
        session_id: &str,
        reason: &str,
    ) -> Result<Finding>;
    /// Checks that each cited tool call is a successful evidence-producing
    /// result in this workspace, produced after the finding was recorded and,
    /// when a role is required, by that role. Returns each result's digest.
    fn finding_validate_tool_evidence(
        &self,
        workspace_id: &str,
        ids: &[String],
        binding: &EvidenceBinding<'_>,
    ) -> Result<Vec<EvidenceDigest>>;
}

impl FindingStore for HostDatabase {
    fn finding_record_tool_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        source: &EvidenceSource<'_>,
    ) -> Result<()> {
        ensure!(
            !source.is_error
                && !source.output.is_empty()
                && source.output.len() <= 8_000_000
                && EVIDENCE_TOOLS.contains(&source.name),
            "only successful read, host command tool, or verified findings_verify output can validate a finding"
        );
        ensure!(
            !workspace_id.is_empty()
                && !session_id.is_empty()
                && session_id.len() <= 128
                && !source.id.is_empty()
                && source.id.len() <= 128
                && source.agent_role.len() <= 64
                && source.created_at_ms >= 0,
            "invalid finding evidence source"
        );
        let hash = format!("{:x}", Sha256::digest(source.output.as_bytes()));
        self.with_connection(|connection| {
            let session: i64 = connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND workspace_id=?2)", params![session_id,workspace_id], |row|row.get(0))?;
            ensure!(session==1,"finding evidence session not found in workspace");
            connection.execute("INSERT OR IGNORE INTO finding_tool_sources(id,workspace_id,session_id,output_sha256,created_at_ms,agent_role,tool_created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![source.id,workspace_id,session_id,hash,now_ms()? as i64,source.agent_role,source.created_at_ms])?;
            let saved: String = connection.query_row("SELECT output_sha256 FROM finding_tool_sources WHERE id=?1 AND workspace_id=?2",params![source.id,workspace_id],|row|row.get(0))?;
            ensure!(saved==hash,"finding evidence source ID conflict");
            Ok(())
        })
    }

    fn finding_get(&self, workspace_id: &str, id: &str) -> Result<Option<Finding>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT record_json FROM findings WHERE workspace_id=?1 AND id=?2",
                    params![workspace_id, id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|json| serde_json::from_str(&json).context("decode finding"))
                .transpose()
        })
    }

    fn finding_get_page(
        &self,
        workspace_id: &str,
        id: &str,
        history_offset: Option<u32>,
    ) -> Result<Option<GetResponse>> {
        self.with_connection(|connection| {
            let raw: Option<String> = connection.query_row(
                "SELECT record_json FROM findings WHERE workspace_id=?1 AND id=?2",
                params![workspace_id,id],
                |row| row.get(0),
            ).optional()?;
            let Some(raw) = raw else { return Ok(None); };
            let mut record: Finding = serde_json::from_str(&raw).context("decode finding")?;
            let history_total: i64 = connection.query_row(
                "SELECT COUNT(*) FROM finding_revisions WHERE workspace_id=?1 AND id=?2",
                params![workspace_id,id],
                |row| row.get(0),
            )?;
            let history_total: usize = history_total.try_into()?;
            let revision = if let Some(offset) = history_offset {
                connection.query_row(
                    "SELECT number,at_ms,session_id,reason,before_json,after_json FROM finding_revisions WHERE workspace_id=?1 AND id=?2 ORDER BY number DESC LIMIT 1 OFFSET ?3",
                    params![workspace_id,id,i64::from(offset)],
                    |row| Ok(Revision {
                        number: row.get::<_,i64>(0)? as u64,
                        at_ms: row.get::<_,i64>(1)? as u64,
                        session_id: row.get(2)?,
                        reason: row.get(3)?,
                        before_json: row.get(4)?,
                        after_json: row.get(5)?,
                    }),
                ).optional()?
            } else { None };
            let next_history_offset = match history_offset {
                None if history_total > 0 => Some(0),
                Some(offset) if revision.is_some() && (offset as usize + 1) < history_total => Some(offset + 1),
                _ => None,
            };
            record.history = revision.into_iter().collect();
            Ok(Some(GetResponse { record, history_total, next_history_offset }))
        })
    }

    fn finding_query(&self, request: &QueryRequest) -> Result<QueryPage> {
        ensure!(
            !request.workspace_id.is_empty()
                && request.text.len() <= 200
                && (1..=50).contains(&request.limit)
                && request.surface_ids.len() <= 16
                && request
                    .surface_ids
                    .iter()
                    .all(|id| !id.is_empty() && id.len() <= 32),
            "invalid finding query"
        );
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT record_json FROM findings WHERE workspace_id=?1 ORDER BY updated_at_ms DESC,id DESC")?;
            let rows = statement.query_map([&request.workspace_id], |row| row.get::<_, String>(0))?;
            let needle = request.text.to_lowercase();
            let mut result = Vec::new();
            let mut total = 0_usize;
            for row in rows {
                let finding: Finding = serde_json::from_str(&row?)?;
                let searchable = [
                    finding.title.as_str(),
                    finding.summary.as_str(),
                    finding.state.candidate.hypothesis.as_str(),
                    finding.root_cause.as_ref().map_or("", |root| root.summary.as_str()),
                    finding.attack_path.as_ref().map_or("", |path| path.summary.as_str()),
                    finding.attack_path.as_ref().map_or("", |path| path.reachability.summary.as_str()),
                    finding.remediation.as_deref().unwrap_or(""),
                ].join(" ").to_lowercase();
                if request.status.is_some_and(|status| status != finding.status())
                    || !request.surface_ids.iter().all(|id| finding.surface_ids.contains(id))
                    || (!needle.is_empty() && !searchable.contains(&needle)) { continue; }
                if total >= request.offset as usize && result.len() < request.limit as usize {
                    result.push(Summary::from(&finding));
                }
                total += 1;
            }
            let end = (request.offset as usize).saturating_add(result.len());
            Ok(QueryPage {
                records: result,
                total,
                next_offset: if end < total { Some(end.try_into()?) } else { None },
            })
        })
    }

    fn finding_create(&self, mut finding: Finding, session_id: &str) -> Result<Finding> {
        ensure!(
            finding.id.is_empty() && finding.history.is_empty() && session_id.len() <= 128,
            "invalid finding create"
        );
        let at_ms = now_ms()?;
        finding.created_at_ms = at_ms;
        finding.updated_at_ms = at_ms;
        finding.revision = 1;
        finding.state.candidate.revision = 1;
        finding.state.candidate.completed_at_ms = at_ms;
        validate(&finding)?;
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let registered: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)", [&finding.workspace_id], |row| row.get(0))?;
            ensure!(registered == 1, "workspace is not registered");
            tx.execute("INSERT OR IGNORE INTO finding_counters(workspace_id) VALUES(?1)", [&finding.workspace_id])?;
            let next: i64 = tx.query_row("SELECT next_id FROM finding_counters WHERE workspace_id=?1", [&finding.workspace_id], |row| row.get(0))?;
            tx.execute("UPDATE finding_counters SET next_id=next_id+1 WHERE workspace_id=?1", [&finding.workspace_id])?;
            finding.id = format!("F{next}");
            let revision = Revision {
                number: 1, at_ms, session_id: session_id.to_owned(), reason: "created".to_owned(),
                before_json: "null".to_owned(), after_json: snapshot(&finding)?,
            };
            tx.execute("INSERT INTO findings(workspace_id,id,updated_at_ms,revision,record_json) VALUES(?1,?2,?3,?4,?5)", params![finding.workspace_id, finding.id, at_ms as i64, 1_i64, serde_json::to_string(&finding)?])?;
            insert_revision(&tx, &finding.workspace_id, &finding.id, &revision)?;
            tx.commit()?;
            Ok(finding)
        })
    }

    fn finding_update(
        &self,
        mut finding: Finding,
        expected_revision: u64,
        session_id: &str,
        reason: &str,
    ) -> Result<Finding> {
        ensure!(
            !reason.trim().is_empty() && reason.len() <= 1_000 && session_id.len() <= 128,
            "invalid finding revision reason"
        );
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let prior_json: String = tx.query_row("SELECT record_json FROM findings WHERE workspace_id=?1 AND id=?2", params![finding.workspace_id, finding.id], |row| row.get(0)).optional()?.context("finding not found")?;
            let prior: Finding = serde_json::from_str(&prior_json)?;
            ensure!(prior.revision == expected_revision, "finding revision conflict");
            ensure!(prior.state.rejected.is_none(), "rejected finding cannot be changed");
            ensure!(prior.id == finding.id && prior.workspace_id == finding.workspace_id && prior.created_at_ms == finding.created_at_ms, "finding identity cannot change");
            finding.revision = prior.revision.checked_add(1).context("finding revision overflow")?;
            finding.updated_at_ms = now_ms()?.max(prior.updated_at_ms.saturating_add(1));
            let before_json = snapshot(&prior)?;
            ensure!(finding.history.is_empty(), "finding history is host-owned");
            let revision = Revision { number: finding.revision, at_ms: finding.updated_at_ms, session_id: session_id.to_owned(), reason: reason.to_owned(), before_json, after_json: snapshot(&finding)? };
            validate(&finding)?;
            tx.execute("UPDATE findings SET updated_at_ms=?3,revision=?4,record_json=?5 WHERE workspace_id=?1 AND id=?2", params![finding.workspace_id, finding.id, finding.updated_at_ms as i64, finding.revision as i64, serde_json::to_string(&finding)?])?;
            insert_revision(&tx, &finding.workspace_id, &finding.id, &revision)?;
            tx.commit()?;
            Ok(finding)
        })
    }

    fn finding_validate_tool_evidence(
        &self,
        workspace_id: &str,
        ids: &[String],
        binding: &EvidenceBinding<'_>,
    ) -> Result<Vec<EvidenceDigest>> {
        ensure!(
            !ids.is_empty() && ids.len() <= 32,
            "validation requires tool evidence"
        );
        self.with_connection(|connection| {
            let mut digests = Vec::with_capacity(ids.len());
            for id in ids {
                ensure!(!id.is_empty() && id.len() <= 128, "invalid tool call ID");
                let recorded = connection
                    .query_row(
                        "SELECT output_sha256, agent_role, tool_created_at_ms FROM finding_tool_sources WHERE id=?1 AND workspace_id=?2",
                        params![id, workspace_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                    )
                    .optional()?;
                let (sha256, role, created_at_ms) = match recorded {
                    Some(source) => source,
                    None => {
                        let saved = connection
                            .query_row(
                                "SELECT call.output, call.agent_role, call.created_at_ms FROM session_tool_calls call JOIN sessions session ON session.id=call.session_id WHERE call.id=?1 AND session.workspace_id=?2 AND call.name IN ('read','bash','powershell','findings_verify') AND call.is_error=0 AND call.output<>''",
                                params![id, workspace_id],
                                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                            )
                            .optional()?
                            .with_context(|| format!("validation evidence {id} must cite a durable successful read, host command tool, or verified findings_verify result in this workspace"))?;
                        (format!("{:x}", Sha256::digest(saved.0.as_bytes())), saved.1, saved.2)
                    }
                };
                ensure!(
                    created_at_ms >= binding.not_before_ms,
                    "validation evidence {id} predates this finding; gather evidence after recording the Candidate"
                );
                if let Some(required) = binding.agent_role {
                    ensure!(
                        role == required,
                        "validation evidence {id} was produced by another agent role"
                    );
                }
                digests.push(EvidenceDigest {
                    reference: id.clone(),
                    sha256,
                });
            }
            Ok(digests)
        })
    }
}
