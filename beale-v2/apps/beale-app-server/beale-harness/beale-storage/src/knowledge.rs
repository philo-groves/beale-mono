use super::HostDatabase;
use anyhow::{Context, Result, ensure};
use beale_contracts::knowledge::{Data, Evidence, Kind, QueryRequest, Record, Revision, Summary};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct EvidenceInput {
    pub tool_call_id: String,
    pub summary: String,
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .min(i64::MAX as u128) as u64)
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Memory => "memory",
        Kind::Claim => "claim",
    }
}

fn projection(record: &Record) -> (&str, &str, &str) {
    match &record.data {
        Data::Memory {
            category,
            role,
            support,
        } => (category, role, support),
        Data::Claim { stage, .. } => ("", "", stage),
    }
}

pub(super) fn migrate_revisions(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS knowledge_revisions (
            workspace_id TEXT NOT NULL,
            record_id TEXT NOT NULL,
            number INTEGER NOT NULL,
            at_ms INTEGER NOT NULL,
            actor TEXT NOT NULL,
            session_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            before_json TEXT NOT NULL,
            after_json TEXT NOT NULL,
            PRIMARY KEY(workspace_id,record_id,number)
         );",
    )?;
    let rows = {
        let mut statement =
            transaction.prepare("SELECT workspace_id,id,record_json FROM knowledge_records")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (workspace_id, id, json) in rows {
        let mut record: Record = serde_json::from_str(&json).context("migrate knowledge record")?;
        for revision in &record.history {
            transaction.execute("INSERT OR IGNORE INTO knowledge_revisions(workspace_id,record_id,number,at_ms,actor,session_id,reason,before_json,after_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![workspace_id,id,revision.number as i64,revision.at_ms as i64,revision.actor,revision.session_id,revision.reason,revision.before_json,revision.after_json])?;
        }
        record.history.clear();
        transaction.execute(
            "UPDATE knowledge_records SET record_json=?3 WHERE workspace_id=?1 AND id=?2",
            params![workspace_id, id, serde_json::to_string(&record)?],
        )?;
    }
    transaction.execute_batch("PRAGMA user_version = 31;")?;
    transaction.commit()?;
    Ok(())
}

fn read(tx: &Transaction<'_>, workspace_id: &str, id: &str) -> Result<Option<Record>> {
    let json: Option<String> = tx
        .query_row(
            "SELECT record_json FROM knowledge_records WHERE workspace_id=?1 AND id=?2",
            params![workspace_id, id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = json else {
        return Ok(None);
    };
    let mut record: Record = serde_json::from_str(&json).context("decode knowledge record")?;
    let mut statement = tx.prepare("SELECT number,at_ms,actor,session_id,reason,before_json,after_json FROM knowledge_revisions WHERE workspace_id=?1 AND record_id=?2 ORDER BY number")?;
    record.history = statement
        .query_map(params![workspace_id, id], |row| {
            Ok(Revision {
                number: row.get::<_, i64>(0)? as u64,
                at_ms: row.get::<_, i64>(1)? as u64,
                actor: row.get(2)?,
                session_id: row.get(3)?,
                reason: row.get(4)?,
                before_json: row.get(5)?,
                after_json: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(record))
}

fn validate_text(record: &Record) -> Result<()> {
    ensure!(!record.workspace_id.is_empty(), "workspace is required");
    ensure!(
        !record.title.trim().is_empty() && record.title.len() <= 200,
        "invalid title"
    );
    ensure!(
        record.summary.len() <= 4_000 && record.body.len() <= 16_000,
        "knowledge text is too long"
    );
    ensure!(
        record.surface_id.len() <= 32 && record.links.len() <= 64 && record.evidence.len() <= 128,
        "knowledge record is too large"
    );
    match &record.data {
        Data::Memory {
            category,
            role,
            support,
        } => {
            ensure!(
                matches!(
                    category.as_str(),
                    "observation" | "system_model" | "method" | "prior_art" | "negative_result"
                ),
                "invalid memory category"
            );
            ensure!(
                matches!(
                    role.as_str(),
                    "" | "source" | "sink" | "invariant" | "control" | "trust_boundary"
                ),
                "invalid memory role"
            );
            ensure!(
                role.is_empty() || category == "system_model",
                "memory role requires system_model category"
            );
            ensure!(
                matches!(
                    support.as_str(),
                    "proposed" | "supported" | "disputed" | "stale"
                ),
                "invalid memory support"
            );
            ensure!(
                support != "supported" || !record.evidence.is_empty(),
                "supported memory requires durable evidence"
            );
        }
        Data::Claim {
            stage,
            disposition,
            rating,
            attacker_control,
            violated_invariant,
            observed_behavior,
            impact,
            reproduction_steps,
            source_revision,
            environment,
        } => {
            ensure!(
                matches!(
                    stage.as_str(),
                    "lead" | "observed" | "reproduced" | "verified"
                ),
                "invalid claim stage"
            );
            ensure!(
                matches!(
                    disposition.as_str(),
                    "active" | "refuted" | "stale" | "duplicate"
                ),
                "invalid claim disposition"
            );
            ensure!(
                matches!(
                    rating.as_str(),
                    "informational" | "low" | "medium" | "high" | "critical"
                ),
                "invalid claim rating"
            );
            ensure!(
                impact.len() <= 4_000
                    && attacker_control.len() <= 2_000
                    && violated_invariant.len() <= 2_000
                    && observed_behavior.len() <= 4_000
                    && reproduction_steps.len() <= 8_000
                    && source_revision.len() <= 200
                    && environment.len() <= 200,
                "claim field is too long"
            );
            ensure!(
                stage == "lead" || !record.evidence.is_empty(),
                "observed claim requires durable evidence"
            );
            ensure!(
                stage == "lead"
                    || (!attacker_control.trim().is_empty()
                        && !violated_invariant.trim().is_empty()
                        && !observed_behavior.trim().is_empty()
                        && !impact.trim().is_empty()),
                "observed finding requires attacker control, violated invariant, observed behavior, and impact"
            );
            ensure!(
                stage != "reproduced" && stage != "verified",
                "reproduction and verification require a Knowledge-owned verifier that is not available yet"
            );
            ensure!(
                (disposition == "duplicate") != record.duplicate_of.is_empty(),
                "duplicate disposition requires a canonical claim ID"
            );
        }
    }
    for link in &record.links {
        ensure!(
            link.target_id.len() <= 32
                && !link.target_id.is_empty()
                && link.relation.len() <= 40
                && !link.relation.is_empty()
                && link.note.len() <= 1_000,
            "invalid knowledge link"
        );
        ensure!(link.target_id != record.id, "record cannot link to itself");
        ensure!(
            matches!(
                link.relation.as_str(),
                "supports"
                    | "contradicts"
                    | "context_for"
                    | "depends_on"
                    | "related_to"
                    | "supersedes"
            ),
            "invalid knowledge relation"
        );
    }
    Ok(())
}

fn validate_targets(tx: &Transaction<'_>, record: &Record) -> Result<()> {
    for link in &record.links {
        let target_id = &link.target_id;
        let target = read(tx, &record.workspace_id, target_id)?
            .context("knowledge link target not found in workspace")?;
        if matches!(
            link.relation.as_str(),
            "supports" | "contradicts" | "context_for"
        ) {
            ensure!(
                record.data.kind() == Kind::Memory && target.data.kind() == Kind::Claim,
                "evidence-context links must connect a memory to a claim"
            );
        }
    }
    if !record.duplicate_of.is_empty() {
        ensure!(
            record.duplicate_of != record.id,
            "record cannot duplicate itself"
        );
        let parent = read(tx, &record.workspace_id, &record.duplicate_of)?
            .context("duplicate parent not found")?;
        ensure!(
            parent.data.kind() == record.data.kind() && parent.duplicate_of.is_empty(),
            "duplicate parent must be canonical and of the same kind"
        );
    }
    Ok(())
}

fn attach_evidence(
    tx: &Transaction<'_>,
    record: &mut Record,
    inputs: &[EvidenceInput],
) -> Result<()> {
    ensure!(inputs.len() <= 16, "too many evidence references");
    for input in inputs {
        ensure!(
            !input.tool_call_id.is_empty()
                && input.tool_call_id.len() <= 128
                && !input.summary.trim().is_empty()
                && input.summary.len() <= 1_000,
            "invalid evidence reference"
        );
        if record
            .evidence
            .iter()
            .any(|item| item.tool_call_id == input.tool_call_id)
        {
            continue;
        }
        let receipt: Option<(String, String)> = tx.query_row(
            "SELECT session_id,output_sha256 FROM knowledge_tool_sources WHERE id=?1 AND workspace_id=?2",
            params![input.tool_call_id, record.workspace_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let (session_id, output_sha256) = if let Some(receipt) = receipt {
            receipt
        } else {
            let source: Option<(String, String, String, i64)> = tx.query_row(
                "SELECT call.session_id, call.name, call.output, call.is_error FROM session_tool_calls call JOIN sessions session ON session.id=call.session_id WHERE call.id=?1 AND session.workspace_id=?2",
                params![input.tool_call_id, record.workspace_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).optional()?;
            let (session_id, tool_name, output, is_error) =
                source.context("tool call evidence not found in workspace")?;
            ensure!(
                is_error == 0
                    && !output.is_empty()
                    && matches!(tool_name.as_str(), "read" | "bash"),
                "tool call evidence must be a successful read or bash result"
            );
            (
                session_id,
                format!("{:x}", Sha256::digest(output.as_bytes())),
            )
        };
        let next: i64 = tx.query_row(
            "SELECT next_evidence FROM knowledge_counters WHERE workspace_id=?1",
            [&record.workspace_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE knowledge_counters SET next_evidence=next_evidence+1 WHERE workspace_id=?1",
            [&record.workspace_id],
        )?;
        record.evidence.push(Evidence {
            id: format!("E{next}"),
            tool_call_id: input.tool_call_id.clone(),
            session_id,
            summary: input.summary.clone(),
            output_sha256,
            created_at_ms: now_ms()?,
        });
    }
    Ok(())
}

fn snapshot(record: &Record) -> Result<String> {
    let mut value = serde_json::to_value(record)?;
    if let Some(fields) = value.as_object_mut() {
        for field in ["history", "revision", "created_at_ms", "updated_at_ms"] {
            fields.remove(field);
        }
    }
    Ok(value.to_string())
}

fn persist(tx: &Transaction<'_>, record: &Record) -> Result<()> {
    let (category, role, status) = projection(record);
    let revision = record
        .history
        .last()
        .context("knowledge revision missing")?;
    ensure!(
        revision.number == record.revision,
        "knowledge revision mismatch"
    );
    let mut compact = record.clone();
    compact.history.clear();
    tx.execute("INSERT INTO knowledge_records (workspace_id,id,kind,title,summary,category,role,status,surface_id,duplicate_of,updated_at_ms,revision,record_json)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
        ON CONFLICT(workspace_id,id) DO UPDATE SET title=excluded.title,summary=excluded.summary,category=excluded.category,role=excluded.role,status=excluded.status,surface_id=excluded.surface_id,duplicate_of=excluded.duplicate_of,updated_at_ms=excluded.updated_at_ms,revision=excluded.revision,record_json=excluded.record_json",
        params![record.workspace_id, record.id, kind_name(record.data.kind()), record.title, record.summary, category, role, status, record.surface_id, record.duplicate_of, record.updated_at_ms as i64, record.revision as i64, serde_json::to_string(&compact)?])?;
    tx.execute("INSERT INTO knowledge_revisions(workspace_id,record_id,number,at_ms,actor,session_id,reason,before_json,after_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![record.workspace_id,record.id,revision.number as i64,revision.at_ms as i64,revision.actor,revision.session_id,revision.reason,revision.before_json,revision.after_json])?;
    Ok(())
}

impl HostDatabase {
    pub fn record_knowledge_tool_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        id: &str,
        name: &str,
        output: &str,
        is_error: bool,
    ) -> Result<()> {
        ensure!(
            !is_error && !output.is_empty() && matches!(name, "read" | "bash"),
            "only successful read or bash output can be evidence"
        );
        ensure!(output.len() <= 8_000_000, "evidence output is too large");
        ensure!(
            !workspace_id.is_empty() && session_id.len() <= 128 && id.len() <= 128,
            "invalid tool evidence source"
        );
        let hash = format!("{:x}", Sha256::digest(output.as_bytes()));
        self.with_connection(|connection| {
            let valid_session: i64 = connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND workspace_id=?2)",params![session_id,workspace_id],|row|row.get(0))?;
            ensure!(valid_session == 1, "evidence session not found in workspace");
            connection.execute("INSERT OR IGNORE INTO knowledge_tool_sources(id,workspace_id,session_id,tool_name,output_sha256,output,created_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![id,workspace_id,session_id,name,hash,output,now_ms()? as i64])?;
            let saved_hash: String = connection.query_row("SELECT output_sha256 FROM knowledge_tool_sources WHERE id=?1 AND workspace_id=?2",params![id,workspace_id],|row|row.get(0))?;
            ensure!(saved_hash == hash, "evidence source ID conflict");
            Ok(())
        })
    }

    pub fn knowledge_get(&self, workspace_id: &str, id: &str) -> Result<Option<Record>> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let record = read(&tx, workspace_id, id)?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub fn knowledge_query(&self, input: &QueryRequest) -> Result<Vec<Summary>> {
        ensure!(
            !input.workspace_id.is_empty()
                && input.text.len() <= 200
                && (1..=50).contains(&input.limit),
            "invalid knowledge query"
        );
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT record_json FROM knowledge_records WHERE workspace_id=?1 ORDER BY updated_at_ms DESC,id DESC")?;
            let rows = statement.query_map([&input.workspace_id], |row| row.get::<_, String>(0))?;
            let needle = input.text.to_lowercase();
            let mut result = Vec::new();
            for row in rows {
                let record: Record = serde_json::from_str(&row?)?;
                let summary = Summary::from(&record);
                if input.kind.is_some_and(|kind| kind != summary.kind)
                    || (!input.category.is_empty() && input.category != summary.category)
                    || (!input.role.is_empty() && input.role != summary.role)
                    || (!input.status.is_empty() && input.status != summary.status)
                    || (!input.surface_id.is_empty() && input.surface_id != summary.surface_id)
                    || (!input.include_duplicates && !record.duplicate_of.is_empty())
                    || (!needle.is_empty() && !format!("{} {} {} {}", summary.title, summary.summary, record.body, summary.role).to_lowercase().contains(&needle)) { continue; }
                result.push(summary);
                if result.len() >= input.limit as usize { break; }
            }
            Ok(result)
        })
    }

    pub fn knowledge_create(
        &self,
        workspace_id: &str,
        mut record: Record,
        evidence: &[EvidenceInput],
        actor: &str,
        session_id: &str,
    ) -> Result<Record> {
        ensure!(
            record.workspace_id == workspace_id
                && record.id.is_empty()
                && actor.len() <= 80
                && session_id.len() <= 128,
            "invalid knowledge create"
        );
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let workspace: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)", [workspace_id], |row| row.get(0))?;
            ensure!(workspace == 1, "workspace not registered");
            tx.execute("INSERT OR IGNORE INTO knowledge_counters(workspace_id) VALUES(?1)", [workspace_id])?;
            let (prefix, column) = match record.data.kind() { Kind::Memory => ("M", "next_memory"), Kind::Claim => ("C", "next_claim") };
            let next: i64 = tx.query_row(&format!("SELECT {column} FROM knowledge_counters WHERE workspace_id=?1"), [workspace_id], |row| row.get(0))?;
            tx.execute(&format!("UPDATE knowledge_counters SET {column}={column}+1 WHERE workspace_id=?1"), [workspace_id])?;
            record.id = format!("{prefix}{next}");
            let existing: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM knowledge_records WHERE workspace_id=?1 AND kind=?2 AND lower(trim(title))=lower(trim(?3)) AND duplicate_of='')", params![workspace_id, kind_name(record.data.kind()), record.title], |row|row.get(0))?;
            ensure!(existing == 0, "likely duplicate knowledge title; query and revise the existing record");
            record.revision = 1;
            record.created_at_ms = now_ms()?;
            record.updated_at_ms = record.created_at_ms;
            record.history.clear();
            record.evidence.clear();
            attach_evidence(&tx, &mut record, evidence)?;
            validate_text(&record)?;
            validate_targets(&tx, &record)?;
            record.history.push(Revision { number: 1, at_ms: record.updated_at_ms, actor: actor.to_owned(), session_id: session_id.to_owned(), reason: "created".to_owned(), before_json: "null".to_owned(), after_json: snapshot(&record)? });
            persist(&tx, &record)?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub fn knowledge_update(
        &self,
        mut record: Record,
        expected_revision: u64,
        evidence: &[EvidenceInput],
        actor: &str,
        session_id: &str,
        reason: &str,
    ) -> Result<Record> {
        ensure!(
            !reason.trim().is_empty()
                && reason.len() <= 1_000
                && actor.len() <= 80
                && session_id.len() <= 128,
            "invalid revision reason"
        );
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let prior = read(&tx, &record.workspace_id, &record.id)?
                .context("knowledge record not found")?;
            ensure!(
                prior.revision == expected_revision,
                "knowledge revision conflict"
            );
            ensure!(
                record.data.kind() == prior.data.kind(),
                "knowledge kind cannot change"
            );
            record.created_at_ms = prior.created_at_ms;
            record.updated_at_ms = now_ms()?.max(prior.updated_at_ms.saturating_add(1));
            record.revision = prior
                .revision
                .checked_add(1)
                .context("knowledge revision overflow")?;
            record.history = prior.history.clone();
            record.evidence = prior.evidence.clone();
            attach_evidence(&tx, &mut record, evidence)?;
            validate_text(&record)?;
            validate_targets(&tx, &record)?;
            let before = snapshot(&prior)?;
            let after = snapshot(&record)?;
            ensure!(before != after, "knowledge update contains no changes");
            record.history.push(Revision {
                number: record.revision,
                at_ms: record.updated_at_ms,
                actor: actor.to_owned(),
                session_id: session_id.to_owned(),
                reason: reason.to_owned(),
                before_json: before,
                after_json: after,
            });
            persist(&tx, &record)?;
            tx.commit()?;
            Ok(record)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_record(workspace_id: &str, title: &str, data: Data) -> Record {
        Record {
            id: String::new(),
            workspace_id: workspace_id.to_owned(),
            title: title.to_owned(),
            summary: "Synthetic research context".to_owned(),
            body: String::new(),
            surface_id: String::new(),
            data,
            evidence: Vec::new(),
            links: Vec::new(),
            duplicate_of: String::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            revision: 0,
            history: Vec::new(),
        }
    }

    #[test]
    fn memories_and_claims_keep_distinct_kinds_and_evidence_gates() {
        let root =
            std::env::temp_dir().join(format!("beale-knowledge-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.with_connection(|connection| {
            connection.execute("INSERT INTO workspaces(id,name,directory,ordinal) VALUES ('workspace-example','Example','example-directory',1)", [])?;
            connection.execute("INSERT INTO workspaces(id,name,directory,ordinal) VALUES ('workspace-other','Other','other-directory',2)", [])?;
            connection.execute("INSERT INTO sessions(id,workspace_id,title,created_at_ms) VALUES ('session-example','workspace-example','Example',1)", [])?;
            connection.execute("INSERT INTO sessions(id,workspace_id,title,created_at_ms) VALUES ('session-other','workspace-other','Other',1)", [])?;
            for session in ["session-example", "session-other"] {
                connection.execute("INSERT INTO session_turns(session_id,number,provider,method,model,prompt,answer) VALUES (?1,1,1,1,'model-example','prompt','answer')", [session])?;
            }
            connection.execute("INSERT INTO session_tool_calls(id,session_id,turn_number,ordinal,name,summary,arguments,output,is_error) VALUES ('tool-example','session-example',1,1,'bash','Inspect example','{}','Observed synthetic output',0)", [])?;
            connection.execute("INSERT INTO session_tool_calls(id,session_id,turn_number,ordinal,name,summary,arguments,output,is_error) VALUES ('tool-other','session-other',1,1,'bash','Inspect other','{}','Other synthetic output',0)", [])?;
            Ok(())
        }).unwrap();
        let memory = db
            .knowledge_create(
                "workspace-example",
                example_record(
                    "workspace-example",
                    "Example sensitive sink",
                    Data::Memory {
                        category: "system_model".to_owned(),
                        role: "sink".to_owned(),
                        support: "proposed".to_owned(),
                    },
                ),
                &[],
                "agent",
                "session-example",
            )
            .unwrap();
        assert_eq!(memory.id, "M1");
        assert_eq!(Summary::from(&memory).kind, Kind::Memory);
        let mut supported = memory.clone();
        supported.data = Data::Memory {
            category: "system_model".to_owned(),
            role: "sink".to_owned(),
            support: "supported".to_owned(),
        };
        assert!(
            db.knowledge_update(
                supported.clone(),
                1,
                &[],
                "agent",
                "session-example",
                "Evidence checked"
            )
            .is_err()
        );
        assert!(
            db.knowledge_update(
                supported.clone(),
                1,
                &[EvidenceInput {
                    tool_call_id: "tool-other".to_owned(),
                    summary: "Other workspace result".to_owned()
                }],
                "agent",
                "session-example",
                "Evidence checked"
            )
            .is_err()
        );
        let supported = db
            .knowledge_update(
                supported,
                1,
                &[EvidenceInput {
                    tool_call_id: "tool-example".to_owned(),
                    summary: "Synthetic tool observation".to_owned(),
                }],
                "agent",
                "session-example",
                "Evidence checked",
            )
            .unwrap();
        assert_eq!(supported.revision, 2);
        assert_eq!(supported.evidence[0].id, "E1");
        assert_eq!(supported.history.len(), 2);
        assert!(
            db.knowledge_update(
                supported.clone(),
                1,
                &[],
                "agent",
                "session-example",
                "Stale edit"
            )
            .is_err()
        );
        let claim = db
            .knowledge_create(
                "workspace-example",
                example_record(
                    "workspace-example",
                    "Example authorization candidate",
                    Data::Claim {
                        stage: "lead".to_owned(),
                        disposition: "active".to_owned(),
                        rating: "low".to_owned(),
                        attacker_control: String::new(),
                        violated_invariant: String::new(),
                        observed_behavior: String::new(),
                        impact: String::new(),
                        reproduction_steps: String::new(),
                        source_revision: String::new(),
                        environment: String::new(),
                    },
                ),
                &[],
                "agent",
                "session-example",
            )
            .unwrap();
        assert_eq!(claim.id, "C1");
        let mut observed = claim.clone();
        if let Data::Claim {
            stage,
            attacker_control,
            violated_invariant,
            observed_behavior,
            impact,
            ..
        } = &mut observed.data
        {
            *stage = "observed".to_owned();
            *attacker_control = "Synthetic actor controls a request parameter".to_owned();
            *violated_invariant = "Example authorization boundary".to_owned();
            *observed_behavior = "Synthetic unauthorized response".to_owned();
            *impact = "Example data exposure".to_owned();
        }
        assert!(
            db.knowledge_update(
                observed.clone(),
                1,
                &[],
                "agent",
                "session-example",
                "Observed"
            )
            .is_err()
        );
        let observed = db
            .knowledge_update(
                observed,
                1,
                &[EvidenceInput {
                    tool_call_id: "tool-example".to_owned(),
                    summary: "Synthetic direct observation".to_owned(),
                }],
                "agent",
                "session-example",
                "Observed",
            )
            .unwrap();
        assert_eq!(observed.id, "C1");
        assert_eq!(Summary::from(&observed).kind, Kind::Claim);
        db.record_knowledge_tool_source(
            "workspace-example",
            "session-example",
            "tool-current-turn",
            "read",
            "Current-turn synthetic observation",
            false,
        )
        .unwrap();
        assert!(
            db.knowledge_create(
                "workspace-example",
                example_record(
                    "workspace-example",
                    "Example sink alone",
                    Data::Claim {
                        stage: "observed".to_owned(),
                        disposition: "active".to_owned(),
                        rating: "medium".to_owned(),
                        attacker_control: String::new(),
                        violated_invariant: String::new(),
                        observed_behavior: "Synthetic sensitive call exists".to_owned(),
                        impact: String::new(),
                        reproduction_steps: String::new(),
                        source_revision: String::new(),
                        environment: String::new(),
                    },
                ),
                &[EvidenceInput {
                    tool_call_id: "tool-current-turn".to_owned(),
                    summary: "Synthetic sink inspection".to_owned(),
                }],
                "agent",
                "session-example",
            )
            .is_err()
        );
        let direct = db
            .knowledge_create(
                "workspace-example",
                example_record(
                    "workspace-example",
                    "Example second finding",
                    Data::Claim {
                        stage: "observed".to_owned(),
                        disposition: "active".to_owned(),
                        rating: "medium".to_owned(),
                        attacker_control: "Synthetic actor controls a request parameter".to_owned(),
                        violated_invariant: "Example authorization boundary".to_owned(),
                        observed_behavior: "Synthetic unauthorized response".to_owned(),
                        impact: "Example data exposure".to_owned(),
                        reproduction_steps: String::new(),
                        source_revision: String::new(),
                        environment: String::new(),
                    },
                ),
                &[EvidenceInput {
                    tool_call_id: "tool-current-turn".to_owned(),
                    summary: "Direct synthetic observation".to_owned(),
                }],
                "agent",
                "session-example",
            )
            .unwrap();
        assert_eq!(direct.id, "C2");
        assert!(
            db.knowledge_create(
                "workspace-example",
                example_record(
                    "workspace-example",
                    "Example authorization candidate",
                    claim.data
                ),
                &[],
                "agent",
                "session-example"
            )
            .is_err()
        );
        let query = db
            .knowledge_query(&QueryRequest {
                workspace_id: "workspace-example".to_owned(),
                limit: 50,
                ..QueryRequest::default()
            })
            .unwrap();
        assert_eq!(query.len(), 3);
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened
                .knowledge_get("workspace-example", "M1")
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
