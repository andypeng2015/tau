//! Incremental requested allocations for compaction observability folds.

use super::*;

impl CompactionChainIndex {
    /// Bounds new backing allocations and cloned identities in `observe`.
    ///
    /// The two response correlations can address the same transaction: combine
    /// their timestamp additions before testing the existing vector capacity.
    /// Existing transaction payloads are never copied or charged.
    pub(crate) fn observe_allocation_charge(&self, event: &Event) -> usize {
        use crate::semantic_persistence::fold_memory::{hash_growth, owned_value, vec_growth};

        let mut observations = [None, None];
        let mut attempt = None;
        let mut charge = 0usize;
        match event {
            Event::ProviderResponseFinished(response) => {
                observations[0] = response
                    .automatic_compaction_decision
                    .as_ref()
                    .map(|decision| &decision.transaction_id);
                observations[1] = self.prompts.get(&response.agent_prompt_id);
                if let Some(id) = observations[1] {
                    // Lookup clone, in addition to observe_timed's entry key.
                    charge = charge.saturating_add(owned_value(id));
                }
            }
            Event::AgentPromptTerminated(terminated) => {
                observations[0] = terminated
                    .automatic_compaction_decision
                    .as_ref()
                    .map(|decision| &decision.transaction_id);
            }
            Event::AgentStandaloneCompactionStarted(started) => {
                observations[0] = Some(&started.transaction_id);
                charge = charge.saturating_add(hash_growth::<(
                    tau_proto::AgentPromptId,
                    CompactionTransactionId,
                )>(
                    self.prompts.len(),
                    self.prompts.capacity(),
                    usize::from(!self.prompts.contains_key(&started.compact_prompt_id)),
                ));
                charge = charge
                    .saturating_add(owned_value(&started.compact_prompt_id))
                    .saturating_add(owned_value(&started.transaction_id));
                let predecessor = match &started.trigger {
                    StandaloneCompactionTrigger::AutomaticContextRetreat {
                        failed_transaction_id,
                        ..
                    } => Some(failed_transaction_id),
                    _ => started.supersedes.as_ref(),
                };
                if let Some(predecessor) = predecessor {
                    charge = charge.saturating_add(owned_value(predecessor));
                }
            }
            Event::AgentPromptStarted(started) => {
                observations[0] = self.prompts.get(&started.agent_prompt_id);
                if let Some(id) = observations[0] {
                    charge = charge.saturating_add(owned_value(id));
                }
            }
            Event::ProviderStandaloneExecutionAccounted(accounted) => {
                observations[0] = Some(&accounted.transaction_id);
                attempt = Some(accounted.logical_attempt);
            }
            Event::ProviderStandaloneExecutionAccountingCorrected(corrected) => {
                observations[0] = Some(&corrected.transaction_id);
                attempt = Some(corrected.logical_attempt);
            }
            Event::AgentStandaloneCompactionFailed(failed) => {
                observations[0] = Some(&failed.transaction_id);
            }
            Event::AgentCompacted(compacted) => {
                observations[0] = Some(&compacted.transaction_id);
            }
            Event::AgentInferenceDispatchStarted(checkpoint) => {
                observations[0] = checkpoint.transaction_id.as_ref();
            }
            _ => {}
        }

        let mut new_transactions = 0usize;
        for (index, id) in observations.iter().enumerate() {
            let Some(id) = id else { continue };
            // Every .entry call clones its key, even when the key exists.
            charge = charge.saturating_add(owned_value(*id));
            if index == 1 && observations[0] == Some(*id) {
                continue;
            }
            let additions = if index == 0 && observations[1] == Some(*id) {
                2
            } else {
                1
            };
            if let Some(facts) = self.transactions.get(*id) {
                charge = charge.saturating_add(vec_growth(&facts.timed_facts, additions));
                if let Some(attempt) = attempt {
                    charge = charge.saturating_add(hash_growth::<(ProviderAttempt, AttemptCost)>(
                        facts.attempts.len(),
                        facts.attempts.capacity(),
                        usize::from(!facts.attempts.contains_key(&attempt)),
                    ));
                }
            } else {
                new_transactions = new_transactions.saturating_add(1);
                charge = charge.saturating_add(vec_growth(&Vec::<TimedFact>::new(), additions));
                if attempt.is_some() {
                    charge = charge
                        .saturating_add(hash_growth::<(ProviderAttempt, AttemptCost)>(0, 0, 1));
                }
            }
        }
        charge.saturating_add(hash_growth::<(CompactionTransactionId, TransactionFacts)>(
            self.transactions.len(),
            self.transactions.capacity(),
            new_transactions,
        ))
    }
}
