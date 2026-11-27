//! Leader-owned verifier ledger. Callers authenticate the execution, review its
//! command and commit these operations together with the worker receipt.
//! Accepted artifacts retain transcripts; this ledger retains derived results.
use crate::{FindingStore, ToolContext, VerifyRequest, VerifyRun, transaction_store};
use anyhow::{Context, Result, ensure};
use beale_contracts::{
    findings::{Status, VerificationRun},
    swarm_files::Artifact,
    swarm_findings::{Command, FinishedRun, Observation, VERIFY_RUNS},
    swarm_worker::ExecutionIdentity,
    swarms::valid_id,
};
use beale_storage::rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

struct Saved {
    command: Command,
    session_id: String,
    agent_role: String,
    tool_call_id: String,
    state: String,
    finished: Option<FinishedRun>,
}
fn owned(tx: &Transaction<'_>, identity: &ExecutionIdentity, id: &str) -> Result<Saved> {
    identity.validate().map_err(anyhow::Error::msg)?;
    let (owner, command, session_id, agent_role, tool_call_id, state, finished):
        (String, String, String, String, String, String, Option<String>) = tx.query_row(
        "SELECT identity,command,session_id,agent_role,tool_call_id,state,finished FROM finding_worker_verifications WHERE id=?1",
        [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
    ensure!(
        serde_json::from_str::<ExecutionIdentity>(&owner)? == *identity,
        "verification execution owner mismatch"
    );
    Ok(Saved {
        command: serde_json::from_str(&command)?,
        session_id,
        agent_role,
        tool_call_id,
        state,
        finished: finished.map(|v| serde_json::from_str(&v)).transpose()?,
    })
}

/// Snapshot the request after review, before dispatching either command run.
pub fn prepare(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    session_id: &str,
    agent_role: &str,
    tool_call_id: &str,
    command: &Command,
) -> Result<String> {
    identity.validate().map_err(anyhow::Error::msg)?;
    command.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        valid_id(session_id)
            && valid_id(tool_call_id)
            && !agent_role.is_empty()
            && agent_role.len() <= beale_contracts::swarms::MAX_ID_BYTES,
        "invalid verifier provenance"
    );
    let owner = serde_json::to_string(identity)?;
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM finding_worker_verifications WHERE identity=?1 AND tool_call_id=?2",
            params![owner, tool_call_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        let prior = owned(tx, identity, &id)?;
        ensure!(
            prior.command == *command
                && prior.session_id == session_id
                && prior.agent_role == agent_role,
            "verification preparation changed"
        );
        return Ok(id);
    }
    let context = ToolContext {
        workspace_id: &identity.workspace_id,
        session_id,
        evidence_role: None,
        pending: &[],
    };
    crate::check_verifiable(
        &transaction_store(tx),
        &context,
        &VerifyRequest::parse(&serde_json::to_value(command)?)?,
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO finding_worker_verifications(id,identity,session_id,agent_role,tool_call_id,command,state) VALUES(?1,?2,?3,?4,?5,?6,'running')",
        params![id,owner,session_id,agent_role,tool_call_id,serde_json::to_string(command)?])?;
    Ok(id)
}

/// Accept a published observation, with the host's secret redaction already
/// applied. The caller must resolve the accepted artifact and verify its bytes;
/// metadata from a worker request must never be passed as an accepted artifact.
pub fn observe(
    tx: &Transaction<'_>,
    identity: &ExecutionIdentity,
    artifact: &Artifact,
    observation: &Observation,
) -> Result<()> {
    observation.validate().map_err(anyhow::Error::msg)?;
    artifact.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        observation.identity == *identity
            && artifact.workspace == identity.workspace_id
            && artifact.scan == identity.scan_id
            && artifact.producer == identity.agent_id
            && artifact.attempt == identity.attempt_id
            && artifact.generation == identity.generation,
        "verification artifact provenance mismatch"
    );
    let saved = owned(tx, identity, &observation.run_id)?;
    let result = match &observation.error {
        Some(error) => VerifyRun::failed(error, observation.duration_ms),
        None => VerifyRun::observe(
            &saved.command.oracle,
            observation.exit_code,
            &observation.stdout,
            &observation.stderr,
            observation.duration_ms,
        ),
    };
    let result = serde_json::to_string(&result.0)?;
    let previous: Option<(String,String)> = tx.query_row(
        "SELECT artifact,result FROM finding_worker_observations WHERE run_id=?1 AND ordinal=?2",
        params![observation.run_id, observation.index], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some(previous) = previous {
        ensure!(
            previous == (serde_json::to_string(artifact)?, result),
            "verification observation changed"
        );
        return Ok(());
    }
    let count: u8 = tx.query_row(
        "SELECT COUNT(*) FROM finding_worker_observations WHERE run_id=?1",
        [&observation.run_id],
        |r| r.get(0),
    )?;
    ensure!(
        saved.state == "running" && count == observation.index,
        "verification observation out of order or interrupted"
    );
    tx.execute("INSERT INTO finding_worker_observations(run_id,ordinal,artifact,result) VALUES(?1,?2,?3,?4)",
        params![observation.run_id, observation.index, serde_json::to_string(artifact)?, result])?;
    Ok(())
}

/// Atomically attach two host-evaluated runs to the original finding revision,
/// or retain them as candidate results if that revision is no longer current.
pub fn finish(tx: &Transaction<'_>, identity: &ExecutionIdentity, id: &str) -> Result<FinishedRun> {
    let saved = owned(tx, identity, id)?;
    if let Some(finished) = saved.finished {
        return Ok(finished);
    }
    ensure!(saved.state == "running", "verification was interrupted");
    let rows = tx
        .prepare("SELECT artifact,result FROM finding_worker_observations WHERE run_id=?1 ORDER BY ordinal")?
        .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(
        rows.len() == usize::from(VERIFY_RUNS),
        "verification observations incomplete"
    );
    let artifacts = rows
        .iter()
        .map(|(artifact, _)| serde_json::from_str::<Artifact>(artifact).map(|a| a.id))
        .collect::<Result<Vec<_>, _>>()?;
    let runs = rows
        .into_iter()
        .map(|(_, v)| serde_json::from_str::<VerificationRun>(&v))
        .collect::<Result<Vec<_>, _>>()?;
    let store = transaction_store(tx);
    let current = store.finding_get(&identity.workspace_id, &saved.command.id)?;
    let recorded = current.as_ref().is_some_and(|finding| {
        finding.revision == saved.command.expected_revision
            && matches!(finding.status(), Status::Candidate | Status::Validated)
    });
    let (output, verified) = if recorded {
        let context = ToolContext {
            workspace_id: &identity.workspace_id,
            session_id: &saved.session_id,
            evidence_role: None,
            pending: &[],
        };
        let request = VerifyRequest::parse(&serde_json::to_value(&saved.command)?)?;
        crate::record_verification(
            &store,
            &context,
            &request,
            &saved.tool_call_id,
            &saved.agent_role,
            runs.into_iter().map(VerifyRun).collect(),
        )?
    } else {
        (json!({"verified":false,"recorded":false,"runs":runs,"verification_run":id,"error":"Finding changed during verification; results retained as candidate evidence."}).to_string(), false)
    };
    let mut output: serde_json::Value = serde_json::from_str(&output)?;
    output["verification_run"] = json!(id);
    output["artifact_refs"] = json!(artifacts);
    let finished = FinishedRun {
        run_id: id.into(),
        recorded,
        verified,
        output: output.to_string(),
    };
    tx.execute(
        "UPDATE finding_worker_verifications SET state=?2,finished=?3 WHERE id=?1",
        params![
            id,
            if recorded { "completed" } else { "conflict" },
            serde_json::to_string(&finished)?
        ],
    )?;
    Ok(finished)
}

/// Call only in the supervisor's confirmed-termination transaction. Preserve
/// partial results and completed verifications instead of inventing outcomes.
pub fn interrupt_attempt(tx: &Transaction<'_>, identity: &ExecutionIdentity) -> Result<()> {
    identity.validate().map_err(anyhow::Error::msg)?;
    tx.execute("UPDATE finding_worker_verifications SET state='interrupted' WHERE identity=?1 AND state='running'", [serde_json::to_string(identity)?]).context("interrupt assigned verification")?;
    Ok(())
}

#[cfg(test)]
mod tests;
