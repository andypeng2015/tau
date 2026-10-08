//! Validated scalar evidence for offline compaction-delimited input
//! distributions.
//!
//! This format deliberately leaves endpoint selection to the consumer. Sequence
//! preserves prompt-start ownership even when its terminal follows a boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use serde_json::json;
use tau_core::AgentJournalSnapshot;
use tau_proto::{AgentPromptId, Event};

use crate::{AgentTraceError, InspectError};

/// Writes each selected journal once, retaining only typed correlation
/// identities.
pub(super) fn write_jsonl(
    snapshot: &AgentJournalSnapshot,
    output: &mut impl Write,
) -> Result<(), InspectError> {
    row(
        output,
        json!({
            "schema": "tau.agent_context",
            "schema_version": 0,
            "record_type": "header",
            "content_included": false,
        }),
    )?;
    for agent_id in snapshot.agent_ids() {
        let mut prompts = BTreeMap::<AgentPromptId, (bool, bool)>::new();
        let mut transactions = BTreeSet::new();
        let mut created = false;
        let mut previous_time = None;
        let mut regressions = 0_u64;
        let mut missing_times = 0_u64;
        for record in snapshot.records(agent_id)? {
            let record = record?;
            let at = record.recorded_at.get();
            if at == 0 {
                missing_times += 1;
            } else {
                if previous_time.is_some_and(|previous| at < previous) {
                    regressions += 1;
                }
                previous_time = Some(at);
            }
            let evidence = match record.event {
                Event::AgentStarted(value) => {
                    if created || record.seq.get() != 0 || value.agent_id != *agent_id {
                        return Err(ambiguous("creation"));
                    }
                    created = true;
                    Some(json!({"record_type": "agent_created", "role": value.role}))
                }
                Event::AgentPromptStarted(value) => {
                    let ordinary = value.operation.is_inference();
                    if prompts
                        .insert(value.agent_prompt_id.clone(), (ordinary, false))
                        .is_some()
                    {
                        return Err(ambiguous("prompt start"));
                    }
                    ordinary.then(|| {
                        json!({
                            "record_type": "inference_started",
                            "prompt_id": value.agent_prompt_id,
                            "model": value.model,
                        })
                    })
                }
                Event::ProviderResponseFinished(value) => {
                    match prompts.get_mut(&value.agent_prompt_id) {
                        Some((ordinary, terminal)) => {
                            if *terminal {
                                return Err(ambiguous("prompt terminal"));
                            }
                            *terminal = true;
                            ordinary.then(|| {
                                json!({
                                    "record_type": "inference_finished",
                                    "prompt_id": value.agent_prompt_id,
                                    "input_tokens": value.usage.map(|usage| usage.prompt_sent_tokens),
                                })
                            })
                        }
                        None => return Err(ambiguous("terminal without start")),
                    }
                }
                Event::AgentCompacted(value) => {
                    if value.agent_id != *agent_id
                        || !transactions.insert(value.transaction_id.clone())
                    {
                        return Err(ambiguous("compaction boundary"));
                    }
                    Some(json!({
                        "record_type": "compacted",
                        "transaction_id": value.transaction_id,
                        "model": value.model,
                        "compact_input_tokens": value.original_input_tokens,
                    }))
                }
                _ => None,
            };
            if let Some(mut evidence) = evidence {
                evidence["agent_id"] = json!(agent_id);
                evidence["seq"] = json!(record.seq.get());
                evidence["recorded_at_unix_micros"] = json!(at);
                row(output, evidence)?;
            }
        }
        if !created {
            return Err(ambiguous("missing creation"));
        }
        row(
            output,
            json!({
                "record_type": "journal_summary",
                "agent_id": agent_id,
                "clock_regressions": regressions,
                "missing_timestamps": missing_times,
            }),
        )?;
    }
    Ok(())
}

fn row(output: &mut impl Write, value: serde_json::Value) -> Result<(), InspectError> {
    serde_json::to_writer(&mut *output, &value)
        .map_err(|error| ambiguous(&format!("serialization: {error}")))?;
    writeln!(output)?;
    Ok(())
}

fn ambiguous(fact: &str) -> InspectError {
    InspectError::Trace(AgentTraceError::Projection(format!(
        "ambiguous context evidence: {fact}"
    )))
}
