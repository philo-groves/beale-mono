use super::HostDatabase;
use anyhow::{Context, Result, ensure};
use beale_contracts::findings::{EvidenceRef, Finding, QueryRequest, Status, Summary};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

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

pub(super) fn migrate_state_structure(connection: &mut Connection) -> Result<()> {
    let tx = connection.transaction()?;
    let records = {
        let mut statement = tx.prepare("SELECT workspace_id,id,record_json FROM findings")?;
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
    for (workspace_id, id, raw) in records {
        let mut value: Value =
            serde_json::from_str(&raw).context("decode finding migration record")?;
        let object = value
            .as_object_mut()
            .context("invalid finding migration record")?;
        if object.contains_key("state") {
            continue;
        }
        let old_surface = object
            .remove("surface_id")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        object.insert(
            "surface_ids".to_owned(),
            json!(if old_surface.is_empty() {
                Vec::new()
            } else {
                vec![old_surface]
            }),
        );
        object.insert("summary".to_owned(), json!(""));
        let mut states = serde_json::Map::new();
        for name in ["candidate", "validated", "screened", "reported", "rejected"] {
            let mut state = object.remove(name).unwrap_or(Value::Null);
            if let Some(fields) = state.as_object_mut() {
                let evidence = if name == "validated" {
                    fields.remove("evidence_tool_call_ids").and_then(|value|value.as_array().cloned())
                        .map(|ids|ids.into_iter().filter_map(|id|id.as_str().map(|reference|json!({"kind":"tool_call","reference":reference,"note":null}))).collect::<Vec<_>>())
                } else if name == "screened" {
                    let mut refs = Vec::new();
                    let mut seen = HashSet::new();
                    for check_name in ["validation_review", "known_issue_review", "scope_review"] {
                        if let Some(check) =
                            fields.get_mut(check_name).and_then(Value::as_object_mut)
                            && let Some(old_refs) = check
                                .remove("evidence_refs")
                                .and_then(|value| value.as_array().cloned())
                        {
                            for reference in old_refs.iter().filter_map(Value::as_str) {
                                if seen.insert(reference.to_owned()) {
                                    refs.push(json!({"kind":"other","reference":reference,"note":check_name}));
                                }
                            }
                        }
                    }
                    Some(refs)
                } else {
                    None
                };
                fields.insert(
                    "evidence_refs".to_owned(),
                    evidence
                        .filter(|refs| !refs.is_empty())
                        .map_or(Value::Null, |refs| json!(refs)),
                );
            }
            states.insert(name.to_owned(), state);
        }
        object.insert("state".to_owned(), Value::Object(states));
        let finding: Finding = serde_json::from_value(value).context("migrate finding state")?;
        validate(&finding)?;
        tx.execute(
            "UPDATE findings SET record_json=?3 WHERE workspace_id=?1 AND id=?2",
            params![workspace_id, id, serde_json::to_string(&finding)?],
        )?;
    }
    tx.execute_batch("PRAGMA user_version = 42;")?;
    tx.commit()?;
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
    ensure!(finding.history.len() <= 256, "too many finding revisions");
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

impl HostDatabase {
    pub fn finding_record_tool_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        id: &str,
        name: &str,
        output: &str,
        is_error: bool,
    ) -> Result<()> {
        ensure!(
            !is_error
                && !output.is_empty()
                && output.len() <= 8_000_000
                && matches!(name, "read" | "bash"),
            "only successful read or bash output can validate a finding"
        );
        ensure!(
            !workspace_id.is_empty()
                && !session_id.is_empty()
                && session_id.len() <= 128
                && !id.is_empty()
                && id.len() <= 128,
            "invalid finding evidence source"
        );
        let hash = format!("{:x}", Sha256::digest(output.as_bytes()));
        self.with_connection(|connection| {
            let session: i64 = connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND workspace_id=?2)", params![session_id,workspace_id], |row|row.get(0))?;
            ensure!(session==1,"finding evidence session not found in workspace");
            connection.execute("INSERT OR IGNORE INTO finding_tool_sources(id,workspace_id,session_id,output_sha256,created_at_ms) VALUES(?1,?2,?3,?4,?5)",params![id,workspace_id,session_id,hash,now_ms()? as i64])?;
            let saved: String = connection.query_row("SELECT output_sha256 FROM finding_tool_sources WHERE id=?1 AND workspace_id=?2",params![id,workspace_id],|row|row.get(0))?;
            ensure!(saved==hash,"finding evidence source ID conflict");
            Ok(())
        })
    }

    pub fn finding_get(&self, workspace_id: &str, id: &str) -> Result<Option<Finding>> {
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

    pub fn finding_query(&self, request: &QueryRequest) -> Result<Vec<Summary>> {
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
                result.push(Summary::from(&finding));
                if result.len() >= request.limit as usize { break; }
            }
            Ok(result)
        })
    }

    pub fn finding_create(&self, mut finding: Finding, session_id: &str) -> Result<Finding> {
        ensure!(
            finding.id.is_empty() && session_id.len() <= 128,
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
            finding.history = vec![beale_contracts::findings::Revision {
                number: 1, at_ms, session_id: session_id.to_owned(), reason: "created".to_owned(),
                before_json: "null".to_owned(), after_json: snapshot(&finding)?,
            }];
            tx.execute("INSERT INTO findings(workspace_id,id,updated_at_ms,revision,record_json) VALUES(?1,?2,?3,?4,?5)", params![finding.workspace_id, finding.id, at_ms as i64, 1_i64, serde_json::to_string(&finding)?])?;
            tx.commit()?;
            Ok(finding)
        })
    }

    pub fn finding_update(
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
            finding.history = prior.history;
            finding.history.push(beale_contracts::findings::Revision { number: finding.revision, at_ms: finding.updated_at_ms, session_id: session_id.to_owned(), reason: reason.to_owned(), before_json, after_json: snapshot(&finding)? });
            validate(&finding)?;
            tx.execute("UPDATE findings SET updated_at_ms=?3,revision=?4,record_json=?5 WHERE workspace_id=?1 AND id=?2", params![finding.workspace_id, finding.id, finding.updated_at_ms as i64, finding.revision as i64, serde_json::to_string(&finding)?])?;
            tx.commit()?;
            Ok(finding)
        })
    }

    pub fn finding_validate_tool_evidence(&self, workspace_id: &str, ids: &[String]) -> Result<()> {
        ensure!(
            !ids.is_empty() && ids.len() <= 32,
            "validation requires tool evidence"
        );
        self.with_connection(|connection| {
            for id in ids {
                ensure!(!id.is_empty() && id.len() <= 128, "invalid tool call ID");
                let valid: i64 = connection.query_row("SELECT EXISTS(SELECT 1 FROM finding_tool_sources WHERE id=?1 AND workspace_id=?2) OR EXISTS(SELECT 1 FROM session_tool_calls call JOIN sessions session ON session.id=call.session_id WHERE call.id=?1 AND session.workspace_id=?2 AND call.name IN ('read','bash') AND call.is_error=0 AND call.output<>'')", params![id, workspace_id], |row| row.get(0))?;
                ensure!(valid == 1, "validation evidence must cite a durable successful read or bash result in this workspace");
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_forty_two_migrates_legacy_finding_state_and_evidence() {
        let root =
            std::env::temp_dir().join(format!("beale-findings-migration-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let old = json!({
            "id":"F1","workspace_id":"workspace-example","title":"Example finding",
            "surface_id":"S1","created_at_ms":1,"updated_at_ms":2,"revision":2,
            "history":[{"number":1,"at_ms":1,"session_id":"session-example","reason":"created","before_json":"null","after_json":"{}"}],
            "candidate":{"hypothesis":"Synthetic boundary hypothesis","revision":1,"completed_at_ms":1},
            "validated":{"attacker_control":"Example input","violated_invariant":"Example boundary","observed_behavior":"Synthetic response","impact":"Example impact","evidence_tool_call_ids":["tool-example"],"revision":2,"completion":null},
            "screened":{"validation_review":{"passed":true,"method":"Review","conclusion":"Supported","evidence_refs":["review-example"]},"known_issue_review":{"passed":true,"method":"Review","conclusion":"Unknown","evidence_refs":[]},"scope_review":{"passed":true,"method":"Review","conclusion":"In scope","evidence_refs":["scope-example"]},"revision":2,"completion":null},
            "reported":null,"rejected":null
        });
        db.with_connection(|connection| {
            connection.execute("INSERT INTO findings(workspace_id,id,updated_at_ms,revision,record_json) VALUES(?1,?2,2,2,?3)", params!["workspace-example","F1",old.to_string()])?;
            connection.execute_batch("PRAGMA user_version = 41;")?;
            Ok(())
        }).unwrap();
        drop(db);

        let migrated = HostDatabase::open(&root).unwrap();
        let finding = migrated
            .finding_get("workspace-example", "F1")
            .unwrap()
            .unwrap();
        assert_eq!(finding.summary, "");
        assert_eq!(finding.surface_ids, vec!["S1"]);
        assert!(finding.root_cause.is_none());
        assert!(finding.attack_path.is_none());
        assert!(finding.remediation.is_none());
        assert_eq!(finding.history.len(), 1);
        assert_eq!(finding.history[0].session_id, "session-example");
        assert_eq!(
            finding.state.validated.as_ref().unwrap().validation_method,
            ""
        );
        assert!(
            finding
                .state
                .validated
                .as_ref()
                .unwrap()
                .validation_limitations
                .is_empty()
        );
        assert!(
            finding
                .state
                .validated
                .as_ref()
                .unwrap()
                .counter_evidence
                .is_empty()
        );
        assert_eq!(
            finding
                .state
                .validated
                .as_ref()
                .unwrap()
                .evidence_refs
                .as_ref()
                .unwrap()[0]
                .reference,
            "tool-example"
        );
        let screened = finding.state.screened.unwrap();
        let references = screened.evidence_refs.unwrap();
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].reference, "review-example");
        assert_eq!(references[0].note.as_deref(), Some("validation_review"));
        let version: i64 = migrated
            .with_connection(|connection| {
                Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
            })
            .unwrap();
        assert_eq!(version, 42);
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }
}
