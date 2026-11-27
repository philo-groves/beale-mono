//! Entire bundled cycles driven by model inputs, tool replies and normal scheduling.
use super::*;
use beale_contracts::swarm_local::InboxMessage;
use std::collections::{BTreeMap, HashSet, VecDeque};

fn mails(value: &Value, output: &mut BTreeMap<u64, InboxMessage>) {
    if let Ok(mail) = serde_json::from_value::<InboxMessage>(value.clone()) {
        output.insert(mail.sequence, mail);
        return;
    }
    match value {
        Value::Object(m) => m.values().for_each(|v| mails(v, output)),
        Value::Array(a) => a.iter().for_each(|v| mails(v, output)),
        Value::String(s) => {
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                mails(&v, output);
            }
        }
        _ => (),
    }
}
fn completed(h: &HandoffRequest, assessment: Assessment) -> WorkResult {
    WorkResult {
        completion: Completion::Complete,
        assessment,
        summary: "Synthetic scheduled assessment".into(),
        observations: h.observations.clone(),
        evidence_refs: h.artifact_refs.clone(),
        artifact_refs: h.artifact_refs.clone(),
        unresolved_questions: vec![],
    }
}
enum Step {
    Tool(String, Value),
    Emit(String, String, HandoffPayload, Vec<String>),
    Remediation(String),
    RetainCheck,
}
#[derive(Default)]
struct FlowActor {
    stage: String,
    role: String,
    started: bool,
    children: BTreeMap<String, String>,
    spawning: Option<String>,
    seen: HashSet<String>,
    queue: VecDeque<Step>,
    input: Option<HandoffRequest>,
    last_tool: String,
    finding: String,
    check_output: String,
    blocked: bool,
    released: bool,
    calls: usize,
}
#[derive(Default)]
struct Flow {
    actors: HashMap<String, FlowActor>,
    observed: Vec<String>,
}
impl FlowActor {
    fn tool(&mut self, name: &str, args: Value) {
        self.queue.push_back(Step::Tool(name.into(), args));
    }
    fn complete(&mut self, h: &HandoffRequest, assessment: Assessment) {
        self.tool(
            "swarm_handoff",
            json!({"action":"complete","id":h.id,"result":completed(h, assessment)}),
        );
    }
    fn emit(&mut self, id: &str, to: &str, payload: HandoffPayload, artifacts: Vec<String>) {
        self.queue
            .push_back(Step::Emit(id.into(), to.into(), payload, artifacts));
    }
    fn prepare(&mut self, definition: &str) {
        if self.input.is_none() {
            self.tool(
                "write",
                json!({"path":"evidence.txt","content":"Synthetic scheduled evidence"}),
            );
            if definition == "vdh_vvs" {
                self.emit(
                    "candidate-initial",
                    "validation_system",
                    HandoffPayload::Candidate {
                        claim: "Synthetic candidate needing evidence".into(),
                    },
                    vec!["file:evidence.txt".into()],
                );
            } else {
                for item in ["a", "b"] {
                    self.emit(
                        &format!("rank-{item}"),
                        "discovery",
                        HandoffPayload::RankedTargets {
                            targets: vec![RankedTarget {
                                rank: 1,
                                target: format!("Synthetic item {item}"),
                                rationale: "Example boundary".into(),
                            }],
                        },
                        vec!["file:evidence.txt".into()],
                    );
                }
            }
            return;
        }
        let h = self.input.clone().unwrap();
        for (index, artifact) in h.artifact_refs.iter().enumerate() {
            self.tool(
                "swarm_materialize_artifact",
                json!({"artifact":artifact,"destination":format!("evidence-{index}.txt")}),
            );
            self.tool("read", json!({"path":format!("evidence-{index}.txt")}));
        }
        if definition == "vdh_vvs" {
            match h.id.as_str() {
                "candidate-initial" => {
                    self.complete(&h, Assessment::Inconclusive);
                    self.emit(
                        "missing-evidence",
                        "discovery_harness",
                        HandoffPayload::Task {},
                        h.artifact_refs,
                    );
                }
                "missing-evidence" => {
                    self.tool("write", json!({"path":"supplement.txt","content":"Synthetic supplemental evidence"}));
                    self.complete(&h, Assessment::NotApplicable);
                    let mut refs = h.artifact_refs;
                    refs.push("file:supplement.txt".into());
                    self.emit(
                        "candidate-supplement",
                        "validation_system",
                        HandoffPayload::Candidate {
                            claim: "Synthetic candidate with supplemental evidence".into(),
                        },
                        refs,
                    );
                }
                "candidate-supplement" => {
                    self.complete(&h, Assessment::Supported);
                    self.emit(
                        "final-assessment",
                        "discovery_harness",
                        HandoffPayload::Assessment {
                            result: completed(&h, Assessment::Supported),
                        },
                        h.artifact_refs,
                    );
                }
                "final-assessment" => self.complete(&h, Assessment::NotApplicable),
                _ => panic!("unexpected VDH input"),
            }
            return;
        }
        let item = h.id.rsplit('-').next().unwrap();
        if h.id == "rank-a" && !self.released {
            self.tool("swarm_handoff", json!({"action":"block","id":h.id}));
            self.blocked = true;
            return;
        }
        self.complete(
            &h,
            if self.stage == "validation" {
                Assessment::Supported
            } else {
                Assessment::NotApplicable
            },
        );
        match &h.payload {
            HandoffPayload::RankedTargets { .. } => self.emit(
                &format!("candidate-{item}"),
                "validation",
                HandoffPayload::Candidate {
                    claim: format!("Synthetic candidate {item}"),
                },
                h.artifact_refs,
            ),
            HandoffPayload::Candidate { .. } => self.emit(
                &format!("supported-{item}"),
                "reporter",
                HandoffPayload::SupportedFinding {
                    result: completed(&h, Assessment::Supported),
                },
                h.artifact_refs,
            ),
            HandoffPayload::SupportedFinding { .. } => {
                self.tool("findings_create", json!({"title":format!("Synthetic candidate {item}"),"summary":"Synthetic scheduled evidence","hypothesis":"Example boundary","root_cause":{"summary":"Synthetic explanation","evidence_refs":[{"kind":"artifact","reference":h.artifact_refs[0]}]}}));
                self.queue.push_back(Step::Remediation(item.into()));
            }
            HandoffPayload::Remediation { finding_refs } => {
                self.tool("findings_read", json!({"id":finding_refs[0]}));
                let (patch, test) = if cfg!(windows) {
                    (
                        "(Get-Content input.txt -Raw).Replace('value=old','value=new') | Set-Content input.txt",
                        "if ((Get-Content input.txt -Raw).Trim() -ne 'value=new') { throw 'patch missing' }; Write-Output 'Synthetic patch verified'",
                    )
                } else {
                    (
                        "sed 's/value=old/value=new/' input.txt > patched.txt && mv patched.txt input.txt",
                        "test \"$(cat input.txt)\" = value=new && printf 'Synthetic patch verified'",
                    )
                };
                self.tool("write", json!({"path":patch_name(),"content":patch}));
                self.tool("write", json!({"path":test_name(),"content":test}));
                self.emit(
                    &format!("patch-{item}"),
                    "validation",
                    HandoffPayload::Patch {
                        patch_refs: vec![format!("file:{}", patch_name())],
                        test_refs: vec![format!("file:{}", test_name())],
                    },
                    h.artifact_refs,
                );
            }
            HandoffPayload::Patch {
                patch_refs,
                test_refs,
            } => {
                // A completed assessment must follow the actual independent check.
                let completion = self.queue.pop_back().unwrap();
                for (reference, path) in
                    [(&patch_refs[0], patch_name()), (&test_refs[0], test_name())]
                {
                    self.tool(
                        "swarm_materialize_artifact",
                        json!({"artifact":reference,"destination":path}),
                    );
                }
                self.tool("write", json!({"path":"input.txt","content":"value=old"}));
                self.tool(beale_tools::COMMAND_TOOL_NAME, json!({"command":if cfg!(windows){"& ./check.ps1"}else{"bash check.sh"},"timeout_seconds":10}));
                self.tool(beale_tools::COMMAND_TOOL_NAME, json!({"command":if cfg!(windows){"& ./patch.ps1; & ./check.ps1"}else{"bash patch.sh && bash check.sh"},"timeout_seconds":10}));
                self.queue.push_back(Step::RetainCheck);
                self.queue.push_back(completion);
                let mut result = completed(&h, Assessment::Supported);
                result.artifact_refs = vec!["file:check-output.txt".into()];
                result.evidence_refs = patch_refs.iter().chain(test_refs).cloned().collect();
                for (to, label) in [("patch_generation", "builder"), ("reporter", "reporter")] {
                    self.emit(
                        &format!("patch-result-{label}-{item}"),
                        to,
                        HandoffPayload::PatchAssessment {
                            patch_refs: patch_refs.clone(),
                            test_refs: test_refs.clone(),
                            result: result.clone(),
                        },
                        h.artifact_refs.clone(),
                    );
                }
            }
            HandoffPayload::PatchAssessment { .. } if self.stage == "reporter" && item == "b" => {
                self.tool(
                    "swarm_message_parent",
                    json!({"message":"flow-item-b-complete","handoff":null}),
                );
            }
            HandoffPayload::PatchAssessment { .. } => (),
            _ => panic!("unexpected OAI payload"),
        }
    }
}
fn patch_name() -> &'static str {
    if cfg!(windows) {
        "patch.ps1"
    } else {
        "patch.sh"
    }
}
fn test_name() -> &'static str {
    if cfg!(windows) {
        "check.ps1"
    } else {
        "check.sh"
    }
}

impl Flow {
    fn reply(&mut self, definition: &str, body: Value) -> Value {
        let system = body["messages"][0]["content"].as_str().unwrap();
        if system.contains("host-side reviewer") {
            return json!({"content":json!({"decision":"approved","reason":"Synthetic acceptance fixture"}).to_string()});
        }
        let id = system
            .split("Swarm agent identity: ")
            .nth(1)
            .unwrap()
            .split('.')
            .next()
            .unwrap();
        let parent = system
            .split("Immediate parent identity: ")
            .nth(1)
            .map(|s| s.split('.').next().unwrap())
            .unwrap_or("");
        let actor = self.actors.entry(id.into()).or_insert_with(|| FlowActor {
            role: if system.contains("Role: Leader") {
                "leader"
            } else if system.contains("Role: Deputy") {
                "deputy"
            } else {
                "worker"
            }
            .into(),
            stage: field(&body["messages"], "flow_stage")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            ..Default::default()
        });
        let text = body["messages"].to_string();
        if !actor.last_tool.is_empty() {
            let tool = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|m| m["role"] == "tool")
                .unwrap();
            let output = tool["content"].as_str().unwrap();
            assert!(
                !output.contains("Swarm request rejected") && !output.contains("Tool error:"),
                "{}: {output}",
                actor.last_tool
            );
            if actor.last_tool == "findings_create" {
                actor.finding = field(
                    &serde_json::from_str::<Value>(output)
                        .unwrap_or_else(|e| panic!("finding reply: {e}: {output}")),
                    "id",
                )
                .unwrap()
                .as_str()
                .unwrap()
                .into();
            }
            if actor.last_tool == beale_tools::COMMAND_TOOL_NAME {
                if actor.check_output.is_empty() {
                    assert!(!output.contains("Synthetic patch verified"), "{output}");
                } else {
                    assert!(output.contains("Synthetic patch verified"), "{output}");
                }
                actor.check_output.push_str(output);
            }
            actor.last_tool.clear();
        }
        if let Some(key) = actor.spawning.take() {
            actor.children.insert(
                key,
                field(&body["messages"], "child_id")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
            );
        }
        let mut mail = BTreeMap::new();
        mails(&body["messages"], &mut mail);
        if actor.role == "leader" {
            let stages = if definition == "vdh_vvs" {
                vec!["discovery_harness", "validation_system"]
            } else {
                vec![
                    "audit_targets",
                    "discovery",
                    "validation",
                    "reporter",
                    "patch_generation",
                ]
            };
            if actor.children.len() < stages.len() {
                let stage = stages[actor.children.len()];
                actor.spawning = Some(stage.into());
                actor.tool("swarm_spawn_subagent",json!({"profile":{"id":stage,"revision":1},"task":{"question":json!({"flow_stage":stage}).to_string(),"completion_criteria":["Route synthetic work"],"evidence_refs":[]},"independent":false}));
            } else {
                for m in mail.values() {
                    if let Some(h) = &m.handoff
                        && m.handoff_result.is_none()
                        && actor.seen.insert(h.id.clone())
                    {
                        actor.tool("swarm_handoff",json!({"action":"forward","id":h.id,"to":actor.children[&h.destination_profile.id]}));
                    }
                }
                if definition == "oai_948"
                    && text.contains("flow-item-b-complete")
                    && !actor.released
                {
                    actor.released = true;
                    actor.tool(
                        "swarm_message_subagent",
                        json!({"child_id":actor.children["discovery"],"message":"flow-release-a"}),
                    );
                }
            }
        } else if actor.role == "deputy" {
            let seed = if definition == "vdh_vvs" {
                "discovery_harness"
            } else {
                "audit_targets"
            };
            if !actor.started && actor.stage == seed {
                actor.spawning = Some("seed".into());
                actor.tool("swarm_spawn_subagent",json!({"profile":{"id":"investigator","revision":1},"task":{"question":json!({"flow_stage":actor.stage,"flow_input":"seed"}).to_string(),"completion_criteria":["Return synthetic work"],"evidence_refs":[]},"independent":false}));
            }
            actor.started = true;
            // One spawn at a time: its reply supplies the child identity to this actor.
            if actor.queue.is_empty() {
                for m in mail.values() {
                    let Some(h) = &m.handoff else { continue };
                    if m.handoff_result.is_some() || !actor.seen.insert(h.id.clone()) {
                        continue;
                    }
                    if h.parent_id == id {
                        actor.tool(
                            "swarm_handoff",
                            json!({"action":"forward","id":h.id,"to":parent}),
                        );
                    } else {
                        let independent = actor.stage.starts_with("validation");
                        let profile = if independent {
                            "challenger"
                        } else if actor.stage == "patch_generation" {
                            "builder"
                        } else {
                            "investigator"
                        };
                        actor.spawning = Some(h.id.clone());
                        actor.tool("swarm_spawn_subagent",json!({"profile":{"id":profile,"revision":1},"task":{"question":json!({"flow_stage":actor.stage,"flow_input":h.id}).to_string(),"completion_criteria":["Return evidence-backed assessment"],"evidence_refs":h.artifact_refs},"independent":independent,"handoff_id":h.id}));
                    }
                    break;
                }
            }
            if actor.stage == "reporter" && text.contains("flow-item-b-complete") && !actor.released
            {
                actor.released = true;
                actor.tool(
                    "swarm_message_parent",
                    json!({"message":"flow-item-b-complete","handoff":null}),
                );
            }
            if actor.stage == "discovery" && text.contains("flow-release-a") && !actor.released {
                actor.released = true;
                actor.tool(
                    "swarm_message_subagent",
                    json!({"child_id":actor.children["rank-a"],"message":"flow-release-a"}),
                );
            }
        } else {
            if !actor.started {
                let key = field(&body["messages"], "flow_input")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned();
                if key != "seed" {
                    actor.input = Some(
                        envelope(&body, &key)
                            .expect("first worker task contains the canonical handoff"),
                    );
                }
                actor.started = true;
                actor.prepare(definition);
            } else if actor.blocked && text.contains("flow-release-a") && !actor.released {
                actor.released = true;
                self.observed.push("resumed-a".into());
                let h = actor.input.clone().unwrap();
                actor.complete(&h, Assessment::NotApplicable);
                actor.emit(
                    "candidate-a",
                    "validation",
                    HandoffPayload::Candidate {
                        claim: "Synthetic candidate a".into(),
                    },
                    h.artifact_refs,
                );
            }
        }
        let Some(step) = actor.queue.pop_front() else {
            return json!({"content":"Synthetic turn complete; await parent-mediated work."});
        };
        let (name, args) = match step {
            Step::Tool(name, args) => {
                if name == "swarm_handoff" && args["action"] == "block" {
                    self.observed.push("blocked-a".into());
                }
                (name, args)
            }
            Step::RetainCheck => (
                "write".into(),
                json!({"path":"check-output.txt","content":actor.check_output}),
            ),
            Step::Remediation(item) => {
                assert!(!actor.finding.is_empty());
                actor.queue.push_front(Step::Emit(
                    format!("remediation-{item}"),
                    "patch_generation".into(),
                    HandoffPayload::Remediation {
                        finding_refs: vec![actor.finding.clone()],
                    },
                    actor.input.as_ref().unwrap().artifact_refs.clone(),
                ));
                ("findings_read".into(), json!({"id":actor.finding}))
            }
            Step::Emit(key, to, payload, artifacts) => {
                self.observed.push(key.clone());
                let workflow = beale_swarms::swarm_definition(definition)
                    .unwrap()
                    .snapshot
                    .workflow()
                    .unwrap();
                let route = workflow
                    .routes
                    .iter()
                    .find(|r| r.from == actor.stage && r.to == to && r.payload == payload.kind())
                    .unwrap();
                let observations = actor
                    .input
                    .as_ref()
                    .map(|h| h.observations.clone())
                    .unwrap_or_else(|| vec!["Original synthetic observation".into()]);
                let h = HandoffRequest {
                    id: key,
                    from_agent: id.into(),
                    parent_id: parent.into(),
                    route_id: route.id.clone(),
                    destination_profile: beale_contracts::swarms::ProfileRef {
                        id: to,
                        revision: 1,
                    },
                    task: beale_contracts::swarms::Task {
                        question: "Continue synthetic work".into(),
                        completion_criteria: vec!["Return evidence and limitations".into()],
                        evidence_refs: vec!["evidence-example".into()],
                    },
                    scope: vec!["Synthetic example.test".into()],
                    record_revisions: vec![],
                    artifact_refs: artifacts,
                    observations,
                    unresolved_questions: vec![],
                    payload,
                };
                (
                    "swarm_message_parent".into(),
                    json!({"message":"Synthetic scheduled handoff","handoff":h}),
                )
            }
        };
        actor.calls += 1;
        actor.last_tool = name.clone();
        call(&format!("flow-call-{}", actor.calls), &name, args)
    }
}

async fn run(definition: &str) {
    let flow = Arc::new(std::sync::Mutex::new(Flow::default()));
    let script = flow.clone();
    let name = definition.to_owned();
    let app = Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let script = script.clone();
            let name = name.clone();
            async move {
                let delta = script.lock().unwrap().reply(&name, body);
                let chunk = json!({"choices":[{"delta":delta}]});
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
    let (root, state, mut request, server) = regression_session(app).await;
    #[cfg(feature = "findings")]
    if definition == "oai_948" {
        state
            .db
            .set_feature_enabled(beale_findings::FEATURE_ID, true)
            .unwrap();
        beale_findings::migrate(&state.db).unwrap();
    }
    request.swarm_id = definition.into();
    let detail = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        engine::run_turn(state.clone(), request, None, None),
    )
    .await
    .unwrap()
    .unwrap();
    let scan = detail.summary.unwrap().id;
    let store = beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
    let saved = store.load(&scan, true).unwrap();
    let observed = flow.lock().unwrap().observed.clone();
    assert_eq!(
        saved.handoffs.len(),
        if definition == "vdh_vvs" { 4 } else { 14 },
        "{observed:?}"
    );
    assert!(
        saved
            .handoffs
            .iter()
            .all(|h| h.status == HandoffStatus::Completed),
        "{:?}",
        saved
            .handoffs
            .iter()
            .map(|h| (&h.request.id, h.status))
            .collect::<Vec<_>>()
    );
    assert!(
        saved
            .handoffs
            .iter()
            .all(|h| h.request.observations == vec!["Original synthetic observation"])
    );
    if definition == "vdh_vvs" {
        let original = &saved
            .handoffs
            .iter()
            .find(|h| h.request.id == "candidate-initial")
            .unwrap()
            .request;
        let supplement = &saved
            .handoffs
            .iter()
            .find(|h| h.request.id == "candidate-supplement")
            .unwrap()
            .request;
        assert_eq!(original.artifact_refs[0], supplement.artifact_refs[0]);
        assert_eq!(supplement.artifact_refs.len(), 2);
        let reviewers: Vec<_> = saved
            .handoffs
            .iter()
            .filter(|h| h.request.destination_profile.id == "validation_system")
            .map(|h| h.assignment_context.as_ref().unwrap())
            .collect();
        assert_ne!(reviewers[0], reviewers[1]);
    } else {
        let index = |s: &str| observed.iter().position(|x| x == s).unwrap();
        assert!(index("blocked-a") < index("patch-result-reporter-b"));
        assert!(index("patch-result-reporter-b") < index("resumed-a"));
        for item in ["a", "b"] {
            let patch = saved
                .handoffs
                .iter()
                .find(|h| h.request.id == format!("patch-{item}"))
                .unwrap();
            let HandoffPayload::Patch {
                patch_refs,
                test_refs,
            } = &patch.request.payload
            else {
                panic!("patch")
            };
            for dest in ["builder", "reporter"] {
                let outcome = saved
                    .handoffs
                    .iter()
                    .find(|h| h.request.id == format!("patch-result-{dest}-{item}"))
                    .unwrap();
                let HandoffPayload::PatchAssessment {
                    patch_refs: actual_patch,
                    test_refs: actual_test,
                    result,
                } = &outcome.request.payload
                else {
                    panic!("assessment")
                };
                assert_eq!(actual_patch, patch_refs);
                assert_eq!(actual_test, test_refs);
                assert_ne!(outcome.request.from_agent, patch.request.from_agent);
                assert!(!result.artifact_refs[0].starts_with("file:"));
            }
        }
    }
    let reopened = beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
    assert_eq!(reopened.load(&scan, true).unwrap().handoffs, saved.handoffs);
    server.abort();
    drop(reopened);
    drop(store);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}
#[tokio::test]
async fn vdh_missing_evidence_cycle_runs_through_scheduler_with_provenance() {
    run("vdh_vvs").await;
}
#[cfg(feature = "findings")]
#[tokio::test]
async fn oai_concurrent_full_cycles_run_through_scheduler_and_independent_patch_checks() {
    run("oai_948").await;
}
