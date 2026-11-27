use super::*;
use beale_contracts::swarm_workflows::*;
#[path = "hierarchy_full_workflow_tests.rs"]
mod full_workflows;

// The provider sees only model input. No database reads or fixture-side delivery.
fn field(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .cloned()
            .or_else(|| map.values().find_map(|v| field(v, key))),
        Value::Array(items) => items.iter().rev().find_map(|v| field(v, key)),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| field(&v, key)),
        _ => None,
    }
}
fn envelope(body: &Value, id: &str) -> Option<HandoffRequest> {
    fn visit(v: &Value, id: &str) -> Option<HandoffRequest> {
        if v.get("id").and_then(Value::as_str) == Some(id)
            && let Ok(h) = serde_json::from_value(v.clone())
        {
            return Some(h);
        }
        match v {
            Value::Object(m) => m.values().find_map(|v| visit(v, id)),
            Value::Array(a) => a.iter().rev().find_map(|v| visit(v, id)),
            Value::String(s) => serde_json::from_str::<Value>(s)
                .ok()
                .and_then(|v| visit(&v, id)),
            _ => None,
        }
    }
    visit(&body["messages"], id)
}
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
}
fn spawn(id: &str, profile: &str, question: String, independent: bool) -> Value {
    call(
        id,
        "swarm_spawn_subagent",
        json!({"profile":{"id":profile,"revision":1},"task":{"question":question,"completion_criteria":["Return synthetic evidence"],"evidence_refs":[]},"independent":independent}),
    )
}
#[derive(Default)]
struct Actor {
    phase: usize,
    kind: String,
    child: String,
    other: String,
}
#[derive(Default)]
struct Script {
    actors: HashMap<String, Actor>,
    observed: Vec<String>,
}
impl Script {
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
        let text = body["messages"].to_string();
        let actor = self.actors.entry(id.into()).or_insert_with(|| Actor {
            kind: if system.contains("Role: Leader") {
                "leader"
            } else if system.contains("Role: Deputy") {
                if text.contains("stage10-source-deputy") {
                    "source-deputy"
                } else {
                    "destination-deputy"
                }
            } else if text.contains("stage10-source-worker") {
                "source-worker"
            } else {
                "recipient"
            }
            .into(),
            ..Default::default()
        });
        let (source, destination, payload) = match definition {
            "basic" => (
                "research_deputy",
                "research_deputy",
                HandoffPayload::Task {},
            ),
            "vdh_vvs" => (
                "discovery_harness",
                "validation_system",
                HandoffPayload::Candidate {
                    claim: "Synthetic candidate".into(),
                },
            ),
            _ => (
                "discovery",
                "validation",
                HandoffPayload::Candidate {
                    claim: "Synthetic candidate".into(),
                },
            ),
        };
        let output = match (actor.kind.as_str(), actor.phase) {
            ("leader", 0) => Some(spawn(
                "source-deputy",
                source,
                "stage10-source-deputy".into(),
                false,
            )),
            ("leader", 1) => {
                actor.child = field(&body["messages"], "child_id")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into();
                Some(spawn(
                    "destination-deputy",
                    destination,
                    "stage10-destination-deputy".into(),
                    false,
                ))
            }
            ("leader", 2) => {
                if actor.other.is_empty() {
                    actor.other = field(&body["messages"], "child_id")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .into();
                }
                envelope(&body,"worker-handoff").map(|h| {
                    self.observed.push("leader-received".into());
                    call("forward-recipient","swarm_handoff",json!({"action":"forward","id":h.id,"to":actor.other}))
                })
            }
            ("leader", 3) if text.contains("stage10-received-evidence") => {
                self.observed.push("leader-receipt".into());
                Some(call(
                    "resume-source",
                    "swarm_message_subagent",
                    json!({"child_id":actor.child,"message":"stage10-follow-up"}),
                ))
            }
            ("source-deputy", 0) => Some(spawn(
                "source-worker",
                "investigator",
                "stage10-source-worker".into(),
                false,
            )),
            ("source-deputy", 1) => {
                if actor.child.is_empty() {
                    actor.child = field(&body["messages"], "child_id")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .into();
                }
                envelope(&body, "worker-handoff").map(|h| {
                    assert!(!h.artifact_refs[0].starts_with("file:"));
                    self.observed.push("parent-received".into());
                    call(
                        "forward-parent",
                        "swarm_handoff",
                        json!({"action":"forward","id":h.id,"to":parent}),
                    )
                })
            }
            ("source-deputy", 2) if text.contains("stage10-follow-up") => Some(call(
                "resume-worker",
                "swarm_message_subagent",
                json!({"child_id":actor.child,"message":"stage10-follow-up"}),
            )),
            ("destination-deputy", 0) => envelope(&body, "worker-handoff").map(|_| {
                self.observed.push("destination-received".into());
                call("recipient", "swarm_spawn_subagent", json!({"profile":{"id":"challenger","revision":1},"task":{"question":"stage10-recipient","completion_criteria":["Check evidence"],"evidence_refs":[]},"independent":true,"handoff_id":"worker-handoff"}))
            }),
            ("destination-deputy", 1) if text.contains("handoff_result") && text.contains("Synthetic evidence checked") => Some(call(
                "receipt-parent",
                "swarm_message_parent",
                json!({"message":"stage10-received-evidence"}),
            )),
            ("source-worker", 0) => Some(call(
                "write-evidence",
                "write",
                json!({"path":"evidence.txt","content":"Synthetic routed evidence"}),
            )),
            ("source-worker", 1) => {
                let workflow = beale_swarms::swarm_definition(definition)
                    .unwrap()
                    .snapshot
                    .workflow()
                    .unwrap();
                let route = workflow
                    .routes
                    .iter()
                    .find(|r| {
                        r.from == source && r.to == destination && r.payload == payload.kind()
                    })
                    .unwrap();
                let h = HandoffRequest {
                    id: "worker-handoff".into(),
                    from_agent: id.into(),
                    parent_id: parent.into(),
                    route_id: route.id.clone(),
                    destination_profile: beale_contracts::swarms::ProfileRef {
                        id: destination.into(),
                        revision: 1,
                    },
                    task: beale_contracts::swarms::Task {
                        question: "Check synthetic evidence".into(),
                        completion_criteria: vec!["Return receipt".into()],
                        evidence_refs: vec!["evidence-example".into()],
                    },
                    scope: vec!["Synthetic example.test".into()],
                    record_revisions: vec![],
                    artifact_refs: vec!["file:evidence.txt".into()],
                    observations: vec!["x".repeat(4096); 5],
                    unresolved_questions: vec![],
                    payload,
                };
                Some(call(
                    "handoff",
                    "swarm_message_parent",
                    json!({"message":"Synthetic candidate","handoff":h}),
                ))
            }
            ("source-worker", 2) if text.contains("stage10-follow-up") => {
                self.observed.push("worker-resumed".into());
                Some(call("reread", "read", json!({"path":"evidence.txt"})))
            }
            ("source-worker", 3) => {
                let reply = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "tool")
                    .unwrap();
                assert!(
                    reply["content"]
                        .to_string()
                        .contains("Synthetic routed evidence"),
                    "{reply}"
                );
                self.observed.push("retained-read".into());
                actor.phase += 1;
                None
            }
            ("recipient", 0) if envelope(&body, "worker-handoff").is_some() => {
                let h = envelope(&body, "worker-handoff").unwrap();
                assert!(serde_json::to_vec(&h).unwrap().len() > 16_384);
                assert_eq!(h.observations, vec!["x".repeat(4096); 5]);
                self.observed.push("recipient-scheduled".into());
                Some(call(
                    "materialize",
                    "swarm_materialize_artifact",
                    json!({"artifact":h.artifact_refs[0],"destination":"received.txt"}),
                ))
            }
            ("recipient", 1) => Some(call(
                "read-received",
                "read",
                json!({"path":"received.txt"}),
            )),
            ("recipient", 2) => {
                let reply = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "tool")
                    .unwrap();
                assert!(
                    reply["content"]
                        .to_string()
                        .contains("Synthetic routed evidence"),
                    "{reply}"
                );
                Some(call(
                    "receipt",
                    "swarm_handoff",
                    json!({"action":"complete","id":"worker-handoff","result":{"completion":"complete","assessment":"supported","summary":"Synthetic evidence checked","observations":[],"evidence_refs":envelope(&body,"worker-handoff").unwrap().artifact_refs,"artifact_refs":[],"unresolved_questions":[]}}),
                ))
            }
            _ => None,
        };
        if let Some(output) = output {
            actor.phase += 1;
            output
        } else {
            json!({"content":"Synthetic turn complete; wait for parent messages."})
        }
    }
}

#[tokio::test]
async fn bundled_handoffs_flow_through_parent_turns_scheduler_and_resumed_worker() {
    for definition in ["basic", "vdh_vvs", "oai_948"] {
        let script = Arc::new(std::sync::Mutex::new(Script::default()));
        let provider_script = script.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let script = provider_script.clone();
                async move {
                    let delta = script.lock().unwrap().reply(definition, body);
                    let chunk = json!({"choices":[{"delta":delta}]});
                    (
                        [("content-type", "text/event-stream")],
                        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                    )
                }
            }),
        );
        let (root, state, mut request, server) = regression_session(app).await;
        request.swarm_id = definition.into();
        let detail = tokio::time::timeout(
            std::time::Duration::from_secs(90),
            engine::run_turn(state.clone(), request, None, None),
        )
        .await
        .unwrap()
        .unwrap();
        let observed = script.lock().unwrap().observed.clone();
        assert_eq!(
            observed,
            vec![
                "parent-received",
                "leader-received",
                "destination-received",
                "recipient-scheduled",
                "leader-receipt",
                "worker-resumed",
                "retained-read"
            ],
            "{definition}"
        );
        let scan = detail.summary.unwrap().id;
        let store = beale_storage::swarm_state::SwarmStore::open(state.db.clone()).unwrap();
        let saved = store.load(&scan, true).unwrap();
        assert_eq!(saved.agents.len(), 5);
        assert_eq!(saved.handoffs.len(), 1);
        assert_eq!(saved.handoffs[0].status, HandoffStatus::Completed);
        assert_eq!(
            saved.handoffs[0].result.as_ref().unwrap().assessment,
            Assessment::Supported
        );
        assert_eq!(
            saved.handoffs[0].request.observations,
            vec!["x".repeat(4096); 5]
        );
        let worker = &saved.handoffs[0].request.from_agent;
        assert_eq!(
            saved
                .attempts
                .iter()
                .filter(|a| &a.agent_id == worker)
                .count(),
            2
        );
        assert!(
            saved
                .agents
                .iter()
                .all(|a| a.summary.state == beale_contracts::swarms::AgentState::Completed)
        );
        server.abort();
        drop(store);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}
