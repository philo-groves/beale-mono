use super::*;
use beale_contracts::findings::Oracle;
use beale_storage::{HostDatabase, WorkspaceRow};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    db: HostDatabase,
    identity: ExecutionIdentity,
    command: Command,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("beale-assigned-verifier-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        // Upgrade a pre-verifier Findings database with an existing candidate.
        db.migrate_schema(crate::FEATURE_ID, &crate::MIGRATIONS[..1])
            .unwrap();
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-example".into(),
            name: "Example".into(),
            directory: "example-directory".into(),
        }])
        .unwrap();
        let context = ToolContext {
            workspace_id: "workspace-example",
            session_id: "session-example",
            evidence_role: None,
            pending: &[],
        };
        let value: serde_json::Value = serde_json::from_str(&crate::execute(&db, &context, crate::CREATE,
            &json!({"title":"Synthetic candidate","summary":"Example observation","hypothesis":"Example hypothesis"})).unwrap()).unwrap();
        crate::migrate(&db).unwrap();
        crate::migrate(&db).unwrap();
        Self {
            root,
            db,
            identity: ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: "scan-example".into(),
                agent_id: "agent-example".into(),
                attempt_id: "attempt-example".into(),
                generation: 1,
            },
            command: Command {
                id: value["record"]["id"].as_str().unwrap().into(),
                expected_revision: 1,
                command: "./example-proof".into(),
                oracle: Oracle::StdoutContains {
                    value: "example-observation".into(),
                },
                timeout_seconds: 120,
            },
        }
    }
    fn tx<T>(&self, action: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.db.with_connection(|db| {
            let tx = db.transaction()?;
            let result = action(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }
    fn prepare(&self, call: &str) -> String {
        self.tx(|tx| {
            prepare(
                tx,
                &self.identity,
                "session-example",
                "validation",
                call,
                &self.command,
            )
        })
        .unwrap()
    }
    fn observation(&self, run: &str, index: u8) -> (Artifact, Observation) {
        let observation = Observation {
            identity: self.identity.clone(),
            run_id: run.into(),
            index,
            exit_code: Some(1),
            stdout: "example-observation".into(),
            stderr: String::new(),
            duration_ms: 17,
            error: None,
        };
        let bytes = serde_json::to_vec(&observation).unwrap();
        let artifact = Artifact {
            id: format!("artifact-{run}-{index}"),
            operation: format!("publication-{run}-{index}"),
            workspace: self.identity.workspace_id.clone(),
            scan: self.identity.scan_id.clone(),
            producer: self.identity.agent_id.clone(),
            assignment: Some("assignment-example".into()),
            attempt: self.identity.attempt_id.clone(),
            generation: 1,
            revision: 1,
            source_path: format!("results/run-{index}.json"),
            sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
            size: bytes.len() as u64,
            executable: Some(false),
            previous: None,
        };
        (artifact, observation)
    }
    fn accept(&self, run: &str, index: u8) {
        let (artifact, observation) = self.observation(run, index);
        self.tx(|tx| observe(tx, &self.identity, &artifact, &observation))
            .unwrap();
    }
    fn finding(&self) -> beale_contracts::findings::Finding {
        self.db
            .finding_get(&self.identity.workspace_id, &self.command.id)
            .unwrap()
            .unwrap()
    }
    fn close(self) {
        drop(self.db);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}
use sha2::Digest;

#[test]
fn assigned_verifier_preserves_maximum_saved_profile_in_evidence_provenance() {
    let f = Fixture::new();
    f.db.append_turn_with_tools(
        &beale_storage::SessionRow {
            id: "session-example".into(),
            workspace_id: f.identity.workspace_id.clone(),
            title: "Synthetic session".into(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        },
        &beale_storage::TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "example-model".into(),
            prompt: "Synthetic prompt".into(),
            answer: "Synthetic answer".into(),
            duration_ms: 1,
            internal_prompt: false,
        },
        true,
        &[],
    )
    .unwrap();
    let role = "p".repeat(beale_contracts::swarms::MAX_ID_BYTES);
    let run = f
        .tx(|tx| {
            prepare(
                tx,
                &f.identity,
                "session-example",
                &role,
                "call-example",
                &f.command,
            )
        })
        .unwrap();
    f.accept(&run, 0);
    f.accept(&run, 1);
    let result = f.tx(|tx| finish(tx, &f.identity, &run)).unwrap();
    assert!(result.verified);
    assert_eq!(
        f.finding()
            .state
            .validated
            .unwrap()
            .verification
            .unwrap()
            .agent_role,
        role
    );
    let source = crate::EvidenceSource {
        id: "call-example",
        name: crate::VERIFY,
        output: &result.output,
        is_error: false,
        agent_role: &role,
        created_at_ms: crate::now_ms().unwrap() as i64,
    };
    f.db.finding_record_tool_source(&f.identity.workspace_id, "session-example", &source)
        .unwrap();
    let too_long = format!("{role}x");
    assert!(
        f.db.finding_record_tool_source(
            &f.identity.workspace_id,
            "session-example",
            &crate::EvidenceSource {
                id: "other-call",
                agent_role: &too_long,
                ..source
            }
        )
        .is_err()
    );
    f.close();
}

#[test]
fn assigned_verification_requires_order_and_exact_provenance_and_replays_once() {
    let f = Fixture::new();
    let run = f.prepare("call-example");
    assert_eq!(f.prepare("call-example"), run);
    let mut changed = f.command.clone();
    changed.command = "./different-proof".into();
    assert!(
        f.tx(|tx| prepare(
            tx,
            &f.identity,
            "session-example",
            "validation",
            "call-example",
            &changed
        ))
        .is_err()
    );
    assert!(f.tx(|tx| finish(tx, &f.identity, &run)).is_err());
    let (artifact, observation) = f.observation(&run, 1);
    assert!(
        f.tx(|tx| observe(tx, &f.identity, &artifact, &observation))
            .is_err()
    );
    for field in 0..5 {
        let mut identity = f.identity.clone();
        match field {
            0 => identity.workspace_id = "foreign-workspace".into(),
            1 => identity.scan_id = "foreign-scan".into(),
            2 => identity.agent_id = "foreign-agent".into(),
            3 => identity.attempt_id = "replacement-attempt".into(),
            _ => identity.generation += 1,
        }
        assert!(f.tx(|tx| finish(tx, &identity, &run)).is_err());
        let (mut artifact, mut observation) = f.observation(&run, 0);
        observation.identity = identity.clone();
        artifact.workspace = identity.workspace_id.clone();
        artifact.scan = identity.scan_id.clone();
        artifact.producer = identity.agent_id.clone();
        artifact.attempt = identity.attempt_id.clone();
        artifact.generation = identity.generation;
        assert!(
            f.tx(|tx| observe(tx, &identity, &artifact, &observation))
                .is_err()
        );
    }
    let (mut artifact, mut observation) = f.observation(&run, 0);
    artifact.attempt = "old-attempt".into();
    assert!(
        f.tx(|tx| observe(tx, &f.identity, &artifact, &observation))
            .is_err()
    );
    f.accept(&run, 0);
    f.accept(&run, 0);
    artifact = f.observation(&run, 0).0;
    observation.stdout = "changed result".into();
    assert!(
        f.tx(|tx| observe(tx, &f.identity, &artifact, &observation))
            .is_err()
    );
    f.accept(&run, 1);
    let finished = f.tx(|tx| finish(tx, &f.identity, &run)).unwrap();
    assert!(finished.recorded && finished.verified);
    let output: serde_json::Value = serde_json::from_str(&finished.output).unwrap();
    assert_eq!(
        output["artifact_refs"],
        json!([f.observation(&run, 0).0.id, f.observation(&run, 1).0.id])
    );
    let finding = f.finding();
    assert_eq!(finding.revision, 2);
    let verification = finding.state.validated.unwrap().verification.unwrap();
    assert_eq!(verification.agent_role, "validation");
    assert_eq!(verification.session_id, "session-example");
    assert_eq!(verification.tool_call_id, "call-example");
    assert!(
        verification
            .runs
            .iter()
            .all(|run| run.satisfied && run.duration_ms == 17 && run.output_sha256.len() == 64)
    );
    assert_eq!(f.tx(|tx| finish(tx, &f.identity, &run)).unwrap(), finished);
    assert_eq!(f.finding().revision, 2);
    f.close();
}

#[test]
fn assigned_verification_retains_candidate_results_on_revision_or_status_conflict() {
    for rejected in [false, true] {
        let f = Fixture::new();
        let run = f.prepare("call-example");
        f.accept(&run, 0);
        let context = ToolContext {
            workspace_id: &f.identity.workspace_id,
            session_id: "session-other",
            evidence_role: None,
            pending: &[],
        };
        if rejected {
            crate::execute(
                &f.db,
                &context,
                crate::REJECT,
                &json!({"id":f.command.id,"expected_revision":1,"reason":"Synthetic rejection"}),
            )
            .unwrap();
        } else {
            let mut finding = f.finding();
            finding.summary = "Concurrent synthetic update".into();
            f.db.finding_update(finding, 1, "session-other", "Synthetic concurrent edit")
                .unwrap();
        }
        let edited = f.finding();
        f.accept(&run, 1);
        let result = f.tx(|tx| finish(tx, &f.identity, &run)).unwrap();
        assert!(!result.recorded && !result.verified);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["artifact_refs"].as_array().unwrap().len(), 2);
        assert!(result.output.contains("candidate evidence"));
        assert_eq!(f.finding(), edited);
        assert_eq!(f.tx(|tx| finish(tx, &f.identity, &run)).unwrap(), result);
        f.db.with_connection(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM finding_worker_observations WHERE run_id=?1",
                    [&run],
                    |r| r.get::<_, u32>(0)
                )?,
                2
            );
            Ok(())
        })
        .unwrap();
        f.close();
    }
}

#[test]
fn assigned_verification_rolls_back_with_receipt_and_survives_restart_and_interruption() {
    let mut f = Fixture::new();
    let failure: Result<()> = f.tx(|tx| {
        prepare(
            tx,
            &f.identity,
            "session-example",
            "validation",
            "call-example",
            &f.command,
        )?;
        anyhow::bail!("synthetic receipt failure")
    });
    assert!(failure.is_err());
    f.db.with_connection(|db| {
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM finding_worker_verifications",
                [],
                |r| r.get::<_, u32>(0)
            )?,
            0
        );
        Ok(())
    })
    .unwrap();
    let run = f.prepare("call-example");
    let (artifact, observation) = f.observation(&run, 0);
    let failure: Result<()> = f.tx(|tx| {
        observe(tx, &f.identity, &artifact, &observation)?;
        anyhow::bail!("synthetic receipt failure")
    });
    assert!(failure.is_err());
    f.accept(&run, 0);
    f.accept(&run, 1);
    let failure: Result<()> = f.tx(|tx| {
        finish(tx, &f.identity, &run)?;
        anyhow::bail!("synthetic receipt failure")
    });
    assert!(failure.is_err());
    assert_eq!(f.finding().revision, 1);
    let partial = f.prepare("partial-call");
    f.accept(&partial, 0);
    drop(f.db);
    f.db = HostDatabase::open(&f.root).unwrap();
    crate::migrate(&f.db).unwrap();
    assert_eq!(f.prepare("call-example"), run);
    let finished = f.tx(|tx| finish(tx, &f.identity, &run)).unwrap();
    assert!(finished.verified);
    let failure: Result<()> = f.tx(|tx| {
        interrupt_attempt(tx, &f.identity)?;
        anyhow::bail!("synthetic reconciliation failure")
    });
    assert!(failure.is_err());
    f.tx(|tx| interrupt_attempt(tx, &f.identity)).unwrap();
    assert!(f.tx(|tx| finish(tx, &f.identity, &partial)).is_err());
    let (artifact, observation) = f.observation(&partial, 1);
    assert!(
        f.tx(|tx| observe(tx, &f.identity, &artifact, &observation))
            .is_err()
    );
    assert_eq!(f.tx(|tx| finish(tx, &f.identity, &run)).unwrap(), finished);
    f.db.with_connection(|db| {
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM finding_worker_observations WHERE run_id=?1",
                [&partial],
                |r| r.get::<_, u32>(0)
            )?,
            1
        );
        Ok(())
    })
    .unwrap();
    f.close();
}

#[test]
fn assigned_verifier_evaluates_failures_and_large_transcript_without_truncating_oracle() {
    for fail in [false, true] {
        let f = Fixture::new();
        let run = f.prepare("call-example");
        for index in 0..VERIFY_RUNS {
            let (artifact, mut observation) = f.observation(&run, index);
            observation.stdout = format!("{}example-observation", "x".repeat(1_100_000));
            if fail && index == 1 {
                observation.exit_code = None;
                observation.stdout.clear();
                observation.error = Some("Synthetic collection failure".into());
            }
            f.tx(|tx| observe(tx, &f.identity, &artifact, &observation))
                .unwrap();
        }
        let result = f.tx(|tx| finish(tx, &f.identity, &run)).unwrap();
        assert!(result.recorded);
        assert_eq!(result.verified, !fail);
        let runs = f
            .finding()
            .state
            .validated
            .unwrap()
            .verification
            .unwrap()
            .runs;
        assert!(runs[0].satisfied);
        if fail {
            assert!(!runs[1].satisfied);
            assert_eq!(
                runs[1].error.as_deref(),
                Some("Synthetic collection failure")
            );
        }
        f.close();
    }
}
