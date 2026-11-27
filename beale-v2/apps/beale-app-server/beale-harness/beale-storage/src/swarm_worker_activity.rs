//! Project leader-observed inference into accepted worker session history.
use super::*;
use beale_contracts::swarm_worker::InferenceResult;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    claims: i64,
    completed: i64,
}

pub(super) fn stamp(tx: &Transaction<'_>, turn: &LocalTurn) -> Result<Stamp> {
    let (claims, metadata, completed): (i64, i64, i64) = tx.query_row(
        "SELECT COUNT(*),COUNT(a.sequence),COUNT(a.completed_at_ms) FROM swarm_inference i
         LEFT JOIN swarm_inference_activity a USING(scan_id,agent_id,operation_id)
         WHERE i.scan_id=?1 AND i.agent_id=?2 AND json_extract(i.request,'$.attempt_id')=?3",
        params![turn.scan_id, turn.agent_id, turn.attempt_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    ensure!(
        claims == metadata,
        "worker inference activity metadata missing"
    );
    Ok(Stamp { claims, completed })
}

pub(super) struct Projection {
    pub stamp: Stamp,
    pub reasoning: Vec<crate::ReasoningSummaryRow>,
    pub commentary: Vec<crate::CommentaryRow>,
    pub usage: Vec<crate::ModelUsageRow>,
}

pub(super) fn project(
    tx: &Transaction<'_>,
    turn: &LocalTurn,
    model: &LocalModel,
    route: &str,
    number: i64,
    tool_count: usize,
) -> Result<Projection> {
    let mut out = Projection {
        stamp: stamp(tx, turn)?,
        reasoning: vec![],
        commentary: vec![],
        usage: vec![],
    };
    let mut stmt = tx.prepare("SELECT i.request,i.result,a.started_at_ms,a.completed_at_ms,a.after_tool_count,a.interrupted
        FROM swarm_inference i JOIN swarm_inference_activity a USING(scan_id,agent_id,operation_id)
        WHERE i.scan_id=?1 AND i.agent_id=?2 AND json_extract(i.request,'$.attempt_id')=?3 ORDER BY a.sequence")?;
    let rows = stmt.query_map(params![turn.scan_id, turn.agent_id, turn.attempt_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, bool>(5)?,
        ))
    })?;
    for row in rows {
        let (request, raw, started, completed, after_tool_count, interrupted) = row?;
        let request: WorkerRequest = serde_json::from_str(&request)?;
        checked(request.validate())?;
        ensure!(
            request.generation == turn.generation
                && matches!(&request.operation,
            WorkerOperation::Infer { route_id, .. } if route_id == route),
            "worker inference provenance changed"
        );
        ensure!(
            started >= 0
                && completed.is_none_or(|v| v >= 0)
                && (0..=i64::try_from(tool_count)?).contains(&after_tool_count)
                && raw.is_some() == completed.is_some(),
            "invalid worker inference activity"
        );
        let result = raw
            .map(|raw| serde_json::from_str::<InferenceResult>(&raw))
            .transpose()?;
        if let Some(result) = &result {
            checked(result.validate())?;
        }
        let usage = if let Some(InferenceResult::Completed {
            text,
            calls,
            reasoning_summaries,
            usage,
            ..
        }) = result
        {
            ensure!(!interrupted, "completed inference marked interrupted");
            for text in reasoning_summaries
                .into_iter()
                .filter(|text| !text.trim().is_empty())
            {
                out.reasoning.push(crate::ReasoningSummaryRow {
                    turn_number: number,
                    ordinal: i64::try_from(out.reasoning.len())?,
                    after_tool_count,
                    text,
                    agent_role: turn.profile.clone(),
                });
            }
            if !calls.is_empty() && !text.trim().is_empty() {
                out.commentary.push(crate::CommentaryRow {
                    turn_number: number,
                    ordinal: i64::try_from(out.commentary.len())?,
                    after_tool_count,
                    text,
                    agent_role: turn.profile.clone(),
                });
            }
            usage
        } else {
            None
        };
        let duration = if interrupted {
            0
        } else {
            completed.map_or(0, |end| end.saturating_sub(started).max(0))
        };
        out.usage.push(crate::ModelUsageRow {
            agent_id: turn.agent_id.clone(),
            role: turn.profile.clone(),
            provider: model.provider,
            method: model.method,
            model: model.model.clone(),
            input_tokens: usage
                .as_ref()
                .map(|v| i64::try_from(v.input_tokens))
                .transpose()?,
            output_tokens: usage
                .as_ref()
                .map(|v| i64::try_from(v.output_tokens))
                .transpose()?,
            cache_read_tokens: usage
                .and_then(|v| v.cache_read_tokens)
                .map(i64::try_from)
                .transpose()?,
            duration_ms: duration,
            observed_at_ms: completed.unwrap_or(started),
            // Host observation times do not measure cumulative worker active time.
            work_elapsed_ms: None,
        });
    }
    Ok(out)
}
