//! Stage 4 host routing. Remote execution and worker transport are separate stages.
use super::*;
use anyhow::Context;
use beale_contracts::{
    swarm_files::{Artifact, MaterializeArtifact},
    swarms::Role,
};
use beale_storage::{
    swarm_files::{ResearchFiles, SnapshotInput, hash},
    swarm_state::LocalTurn,
};
use std::path::PathBuf;

pub(super) fn store(state: &SessionState) -> anyhow::Result<ResearchFiles> {
    ResearchFiles::open(state.db.clone(), state.workspaces.root())
}

pub(super) fn capture(state: &SessionState, scan: &str, workspace: &str) -> anyhow::Result<()> {
    store(state)?.capture(
        scan,
        &[SnapshotInput {
            source: state
                .workspaces
                .directory(workspace)
                .map_err(|_| anyhow::anyhow!("workspace unavailable"))?,
            destination: String::new(),
        }],
    )?;
    Ok(())
}

pub(super) fn directory(
    state: &SessionState,
    turn: &LocalTurn,
    leader: PathBuf,
) -> anyhow::Result<PathBuf> {
    if turn.role == Role::Leader {
        Ok(leader)
    } else {
        store(state)?.prepare_agent(&turn.scan_id, &turn.agent_id)
    }
}

pub(super) fn checkpoint(
    state: &SessionState,
    turn: &LocalTurn,
    context: &Value,
) -> anyhow::Result<()> {
    if turn.role != Role::Leader {
        store(state)?.checkpoint(&turn.scan_id, &turn.agent_id, &turn.attempt_id, context)?;
    }
    Ok(())
}

impl tool_runtime::ToolRuntime<'_> {
    /// Only artifact-typed slots are rewritten. Ordinary strings and evidence kinds
    /// remain untouched; canonical IDs are checked against accepted central bytes.
    pub(super) fn attachments(&self, operation: &str, arguments: &mut Value) -> anyhow::Result<()> {
        let turn = self.local.context("no swarm identity")?;
        let files = store(self.state)?;
        let scan = beale_storage::swarm_state::SwarmStore::open(self.state.db.clone())?
            .load(&turn.scan_id, true)?;
        let assignment = if turn.role == Role::Leader {
            None
        } else {
            Some(
                scan.assignments
                    .iter()
                    .find(|a| {
                        a.child_id == turn.agent_id
                            && a.state == beale_contracts::swarms::AssignmentState::Assigned
                    })
                    .map(|a| a.id.clone())
                    .context("research artifact has no active assignment")?,
            )
        };
        let base = Artifact {
            id: String::new(),
            operation: String::new(),
            workspace: self.workspace_id.into(),
            scan: turn.scan_id.clone(),
            producer: turn.agent_id.clone(),
            assignment,
            attempt: turn.attempt_id.clone(),
            generation: turn.generation,
            revision: 1,
            source_path: String::new(),
            sha256: String::new(),
            size: 0,
            executable: None,
            previous: None,
        };
        files.resolve_attachments(self.workspace, operation, &base, arguments)
    }

    pub(super) fn materialize(&self, arguments: Value) -> anyhow::Result<Value> {
        let request: MaterializeArtifact = serde_json::from_value(arguments)?;
        store(self.state)?.materialize_artifact(
            self.workspace_id,
            &request.artifact,
            self.workspace,
            &request.destination,
        )?;
        Ok(
            json!({"artifact":request.artifact,"destination":request.destination,"materialized":true}),
        )
    }
}

pub(super) fn tool_error(error: &anyhow::Error) -> tool_runtime::ToolResult {
    beale_host_log::error!("Swarm file operation rejected: {error:#}");
    tool_runtime::ToolResult::error("Artifact operation was not acknowledged. Verify the workspace-relative file, accepted artifact ID, and unused destination path; retry the same operation after storage is available.".into())
}

pub(super) fn operation(turn: &LocalTurn, call_id: &str) -> String {
    hash(format!("{}:{}:{call_id}", turn.scan_id, turn.attempt_id).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::{swarm_local::*, swarms::*};
    use beale_storage::swarm_state::*;
    use std::path::Path;

    mod workflows {
        include!("research_workflow_tests.rs");
    }

    struct Fixture {
        root: PathBuf,
        workspace: PathBuf,
        state: SessionState,
        swarm: SwarmStore,
        leader: LocalTurn,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_definition("basic")
        }
        fn with_definition(definition: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("beale-file-routing-example-{}", Uuid::new_v4()));
            let workspace = root.join("workspace");
            std::fs::create_dir_all(&workspace).unwrap();
            std::fs::write(workspace.join("shared.txt"), "initial").unwrap();
            let state = build_state(
                "synthetic-token".into(),
                WorkspaceStore::at(root.join("host")).unwrap(),
            )
            .unwrap();
            state
                .db
                .replace_workspaces(&[beale_storage::WorkspaceRow {
                    id: "workspace-example".into(),
                    name: "Example".into(),
                    directory: workspace.to_string_lossy().into(),
                }])
                .unwrap();
            let swarm = SwarmStore::open(state.db.clone()).unwrap();
            let saved = swarm
                .create_scan(
                    ScanConfig {
                        version: VERSION,
                        scan_id: "scan-example".into(),
                        workspace_id: "workspace-example".into(),
                        mode: ScanMode::Swarm {
                            snapshot: Box::new(
                                beale_swarms::swarm_definition(definition)
                                    .unwrap()
                                    .snapshot
                                    .clone(),
                            ),
                        },
                        authorized_scope: "Synthetic fixture".into(),
                        enabled_tools: vec![],
                        provider_routes: vec![],
                        destinations: vec![],
                        pacing: vec![],
                        retention: RetentionPolicy::default(),
                    },
                    true,
                    true,
                )
                .unwrap();
            store(&state)
                .unwrap()
                .capture(
                    "scan-example",
                    &[SnapshotInput {
                        source: workspace.clone(),
                        destination: String::new(),
                    }],
                )
                .unwrap();
            swarm
                .local_initialize(
                    "scan-example",
                    LocalSettings {
                        models: vec![LocalModel {
                            profile: "leader".into(),
                            provider: 1,
                            method: 1,
                            model: "example-model".into(),
                        }],
                        reasoning_effort: 0,
                    },
                    "Coordinate synthetic research",
                )
                .unwrap();
            let leader = swarm
                .local_claim("scan-example", &saved.agents[0].summary.id, 1, 64)
                .unwrap()
                .unwrap();
            Self {
                root,
                workspace,
                state,
                swarm,
                leader,
            }
        }
        fn child(&self, call: &str) -> (LocalTurn, PathBuf) {
            self.spawn(&self.leader, "research_deputy", call, false)
        }
        fn spawn(
            &self,
            parent: &LocalTurn,
            profile: &str,
            call: &str,
            independent: bool,
        ) -> (LocalTurn, PathBuf) {
            let child = self
                .swarm
                .local_dispatch(
                    parent,
                    call,
                    SwarmCommand::Spawn(Spawn {
                        handoff_id: None,
                        profile: ProfileRef {
                            id: profile.into(),
                            revision: 1,
                        },
                        task: Task {
                            question: format!("Inspect synthetic {call}"),
                            completion_criteria: vec!["Return evidence".into()],
                            evidence_refs: vec![],
                        },
                        independent,
                    }),
                    Admission::Ready,
                )
                .unwrap();
            let turn = self
                .swarm
                .local_claim("scan-example", child["child_id"].as_str().unwrap(), 2, 64)
                .unwrap()
                .unwrap();
            let workspace = directory(&self.state, &turn, self.workspace.clone()).unwrap();
            (turn, workspace)
        }
        #[cfg(feature = "findings")]
        fn sql(&self, sql: &str) {
            self.state
                .db
                .with_connection(|db| {
                    db.execute_batch(sql)?;
                    Ok(())
                })
                .unwrap();
        }
        async fn call(
            &self,
            turn: &LocalTurn,
            workspace: &Path,
            call: &str,
            name: &str,
            args: Value,
        ) -> tool_runtime::ToolResult {
            let redactor = beale_tools::Redactor::new(Vec::<String>::new());
            let plugins = HashSet::new();
            let mut definitions = beale_tools::definitions();
            definitions.extend(beale_swarms::local::definitions(turn.role));
            #[cfg(feature = "findings")]
            definitions.extend(beale_findings::definitions());
            let runtime = tool_runtime::ToolRuntime {
                local: Some(turn),
                state: &self.state,
                workspace_id: "workspace-example",
                session_id: &turn.session_id,
                agent_id: &turn.agent_id,
                agent_role: &turn.profile,
                workspace,
                review: tool_runtime::ReviewTarget {
                    provider: Provider::OpenAi,
                    mode: AuthMode::ApiKey,
                    model: "example-model",
                    context: "Synthetic fixture",
                },
                definitions: &definitions,
                callable_plugins: &plugins,
                redactor: &redactor,
                mutation_lock: None,
                progress: None,
            };
            Box::pin(runtime.execute(call, name, Ok(args), true, &[], &mut None)).await
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn native_tools_route_to_private_copies_and_materialize_only_explicitly() {
        let f = Fixture::new();
        let (first, a) = f.child("first-example");
        let (second, b) = f.child("second-example");
        assert_eq!(
            directory(&f.state, &f.leader, f.workspace.clone()).unwrap(),
            f.workspace
        );
        for (turn, path, text) in [(&first, &a, "first result"), (&second, &b, "second result")] {
            let result = f
                .call(
                    turn,
                    path,
                    "write-example",
                    "write",
                    json!({"path":"shared.txt","content":text}),
                )
                .await;
            assert!(!result.is_error, "{}", result.output);
        }
        assert_eq!(
            std::fs::read_to_string(a.join("shared.txt")).unwrap(),
            "first result"
        );
        assert_eq!(
            std::fs::read_to_string(b.join("shared.txt")).unwrap(),
            "second result"
        );
        assert_eq!(
            std::fs::read_to_string(f.workspace.join("shared.txt")).unwrap(),
            "initial"
        );
        let files = store(&f.state).unwrap();
        let artifact = files
            .publish_file(
                &a,
                Artifact {
                    id: "artifact-example".into(),
                    operation: "operation-example".into(),
                    workspace: "workspace-example".into(),
                    scan: first.scan_id.clone(),
                    producer: first.agent_id.clone(),
                    assignment: Some("assignment-example".into()),
                    attempt: first.attempt_id.clone(),
                    generation: first.generation,
                    revision: 1,
                    source_path: "shared.txt".into(),
                    sha256: String::new(),
                    size: 0,
                    executable: None,
                    previous: None,
                },
            )
            .unwrap();
        let result = f
            .call(
                &second,
                &b,
                "materialize-example",
                "swarm_materialize_artifact",
                json!({"artifact":artifact.id,"destination":"handoff.txt"}),
            )
            .await;
        assert!(!result.is_error, "{}", result.output);
        assert_eq!(
            std::fs::read_to_string(b.join("handoff.txt")).unwrap(),
            "first result"
        );
        assert_eq!(
            std::fs::read_to_string(b.join("shared.txt")).unwrap(),
            "second result"
        );
        let result = f
            .call(
                &second,
                &b,
                "overwrite-example",
                "swarm_materialize_artifact",
                json!({"artifact":artifact.id,"destination":"shared.txt"}),
            )
            .await;
        assert!(result.is_error);
        checkpoint(&f.state, &first, &json!({"cursor":1})).unwrap();
        assert!(
            files
                .environment(&first.scan_id, &first.agent_id)
                .unwrap()
                .checkpoint
                .is_some()
        );
        assert_eq!(directory(&f.state, &first, f.workspace.clone()).unwrap(), a);
    }

    #[cfg(feature = "findings")]
    #[tokio::test]
    async fn artifacts_keep_real_assignment_and_attempt_provenance_across_resume() {
        async fn publish(f: &Fixture, turn: &LocalTurn, path: &Path) -> Artifact {
            let created=f.call(turn,path,"candidate-example","findings_create",json!({
                "title":"Synthetic candidate", "summary":"Synthetic evidence", "hypothesis":"Example boundary",
                "root_cause":{"summary":"Candidate explanation","evidence_refs":[{"kind":"artifact","reference":"file:shared.txt"}]}
            })).await;
            assert!(!created.is_error, "{}", created.output);
            let summary: Value = serde_json::from_str(&created.output).unwrap();
            let read = f
                .call(
                    turn,
                    path,
                    "read-example",
                    "findings_read",
                    json!({"id":summary["record"]["id"]}),
                )
                .await;
            let record: Value = serde_json::from_str(&read.output).unwrap();
            let reference = record["record"]["root_cause"]["evidence_refs"][0]["reference"]
                .as_str()
                .unwrap();
            store(&f.state)
                .unwrap()
                .artifact("workspace-example", reference)
                .unwrap()
        }
        let f = Fixture::new();
        beale_findings::migrate(&f.state.db).unwrap();
        let (first, path) = f.child("child-example");
        let original = publish(&f, &first, &path).await;
        f.swarm
            .local_finish(&first, json!({"saved":true}), "Synthetic result", false)
            .unwrap();
        f.swarm
            .local_dispatch(
                &f.leader,
                "resume-example",
                SwarmCommand::MessageChild(MessageChild {
                    child_id: first.agent_id.clone(),
                    message: "Related follow-up".into(),
                }),
                Admission::Ready,
            )
            .unwrap();
        let resumed = f
            .swarm
            .local_claim(&first.scan_id, &first.agent_id, 3, 64)
            .unwrap()
            .unwrap();
        let followup = publish(&f, &resumed, &path).await;
        assert_eq!(followup.producer, original.producer);
        assert_ne!(followup.assignment, original.assignment);
        assert_eq!(followup.attempt, resumed.attempt_id);
        assert_eq!(original.attempt, first.attempt_id);
        assert!(followup.generation > original.generation);
        assert_ne!(followup.id, original.id);
        let leader = publish(&f, &f.leader, &f.workspace).await;
        assert_eq!(leader.assignment, None);
        assert_eq!(leader.attempt, f.leader.attempt_id);
    }

    #[cfg(feature = "findings")]
    #[tokio::test]
    async fn canonical_findings_and_handoffs_wait_for_artifact_and_record_commits() {
        for kind in ["finding", "handoff"] {
            let f = Fixture::new();
            let (turn, workspace) = f.child("child-example");
            beale_findings::migrate(&f.state.db).unwrap();
            let (name, args) = if kind == "finding" {
                (
                    "findings_create",
                    json!({"title":"Synthetic candidate","summary":"Example behavior","hypothesis":"Example boundary","root_cause":{"summary":"Candidate explanation","evidence_refs":[{"kind":"artifact","reference":"file:proof.txt"}]}}),
                )
            } else {
                (
                    "swarm_message_parent",
                    json!({"message":"Synthetic handoff","handoff":{"id":"handoff-example","from_agent":turn.agent_id,"parent_id":f.leader.agent_id,"route_id":"research_deputy-to-research_deputy-task","destination_profile":{"id":"research_deputy","revision":1},"task":{"question":"Assess synthetic output","completion_criteria":["Return evidence"],"evidence_refs":[]},"scope":["Synthetic scope"],"record_revisions":[],"artifact_refs":["file:proof.txt"],"observations":["Candidate bytes"],"unresolved_questions":[],"payload":{"kind":"task"}}}),
                )
            };
            // Calling the canonical stores directly cannot bypass byte acceptance.
            let mut missing = args.clone();
            if kind == "finding" {
                missing["root_cause"]["evidence_refs"][0]["reference"] = json!("artifact-missing");
                let context = beale_findings::ToolContext {
                    workspace_id: "workspace-example",
                    session_id: &turn.session_id,
                    evidence_role: None,
                    pending: &[],
                };
                assert!(beale_findings::execute(&f.state.db, &context, name, &missing).is_err());
            } else {
                missing["handoff"]["artifact_refs"] = json!(["artifact-missing"]);
                let request = serde_json::from_value(missing["handoff"].clone()).unwrap();
                assert!(f.swarm.add_handoff(&turn.scan_id, request).is_err());
                let command =
                    serde_json::from_value(json!({"tool":name,"arguments":missing})).unwrap();
                assert!(
                    f.swarm
                        .local_dispatch(&turn, "bypass-example", command, Admission::Ready)
                        .is_err()
                );
            }
            // Missing local bytes cannot create either canonical record.
            assert!(
                f.call(&turn, &workspace, "attach-example", name, args.clone())
                    .await
                    .is_error
            );
            std::fs::write(workspace.join("proof.txt"), "synthetic candidate bytes").unwrap();
            f.sql("CREATE TRIGGER fail_artifact BEFORE UPDATE OF accepted ON swarm_file_transfers WHEN NEW.accepted=1 BEGIN SELECT RAISE(FAIL,'synthetic disk full'); END;");
            assert!(
                f.call(&turn, &workspace, "attach-example", name, args.clone())
                    .await
                    .is_error
            );
            let files = store(&f.state).unwrap();
            assert_eq!(
                files
                    .environment(&turn.scan_id, &turn.agent_id)
                    .unwrap()
                    .pending_publications
                    .len(),
                1
            );
            f.sql("DROP TRIGGER fail_artifact;");
            f.sql(if kind=="finding" {"CREATE TRIGGER fail_record BEFORE INSERT ON findings BEGIN SELECT RAISE(FAIL,'synthetic record full'); END;"}else{"CREATE TRIGGER fail_record BEFORE UPDATE ON swarm_scans WHEN json_array_length(NEW.document,'$.handoffs')>0 BEGIN SELECT RAISE(FAIL,'synthetic record full'); END;"});
            assert!(
                f.call(&turn, &workspace, "attach-example", name, args.clone())
                    .await
                    .is_error
            );
            assert!(
                files
                    .environment(&turn.scan_id, &turn.agent_id)
                    .unwrap()
                    .pending_publications
                    .is_empty()
            );
            assert!(
                f.swarm
                    .load("scan-example", true)
                    .unwrap()
                    .handoffs
                    .is_empty()
            );
            f.state
                .db
                .with_connection(|db| {
                    assert_eq!(
                        db.query_row("SELECT COUNT(*) FROM findings", [], |r| r.get::<_, i64>(0))?,
                        0
                    );
                    Ok(())
                })
                .unwrap();
            f.sql("DROP TRIGGER fail_record;");
            let result = f
                .call(&turn, &workspace, "attach-example", name, args)
                .await;
            assert!(!result.is_error, "{kind}: {}", result.output);
            let artifact_id = if kind == "finding" {
                let summary: Value = serde_json::from_str(&result.output).unwrap();
                let read = f
                    .call(
                        &turn,
                        &workspace,
                        "read-example",
                        "findings_read",
                        json!({"id":summary["record"]["id"]}),
                    )
                    .await;
                let result: Value = serde_json::from_str(&read.output).unwrap();
                result["record"]["root_cause"]["evidence_refs"][0]["reference"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            } else {
                f.swarm.load("scan-example", true).unwrap().handoffs[0]
                    .request
                    .artifact_refs[0]
                    .clone()
            };
            if kind == "handoff" {
                use beale_contracts::swarm_workflows::{
                    Assessment, Completion, HandoffStatus, WorkResult,
                };
                let change = HandoffChange {
                    id: "handoff-example".into(),
                    expected_revision: 1,
                    status: HandoffStatus::Blocked,
                    assignment_context: None,
                    result: Some(WorkResult {
                        completion: Completion::Blocked,
                        assessment: Assessment::Inconclusive,
                        summary: "Synthetic blocked result".into(),
                        observations: vec![],
                        evidence_refs: vec![],
                        artifact_refs: vec!["artifact-missing".into()],
                        unresolved_questions: vec![],
                    }),
                };
                assert!(
                    f.swarm
                        .change_handoff(&turn.scan_id, "transition-example", change)
                        .is_err()
                );
                assert_eq!(
                    f.swarm.load(&turn.scan_id, true).unwrap().handoffs[0].revision,
                    1
                );
            }
            let accepted = files.artifact("workspace-example", &artifact_id).unwrap();
            assert_eq!(accepted.producer, turn.agent_id);
            let scan = f.swarm.load(&turn.scan_id, true).unwrap();
            let assignment = scan
                .assignments
                .iter()
                .find(|a| a.child_id == turn.agent_id)
                .unwrap();
            assert_eq!(accepted.assignment.as_deref(), Some(assignment.id.as_str()));
            assert_eq!(accepted.attempt, turn.attempt_id);
            assert_eq!(accepted.generation, turn.generation);
        }
    }
}
