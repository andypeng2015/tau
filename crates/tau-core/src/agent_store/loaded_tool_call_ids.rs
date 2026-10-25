//! Exact tool-call identities derived from the trees resident in an agent
//! store.

use std::collections::HashSet;

use tau_proto::{ContextItem, ToolCallId};

use crate::semantic_persistence::fold_memory::hash_growth;
use crate::session::{AgentEntry, AgentNode, AgentTree};

/// Non-persisted exact index of tool-call identities in resident agent trees.
#[derive(Debug, Default)]
pub(super) struct LoadedToolCallIds {
    /// Unique provider-visible identities found in all indexed trees.
    ids: HashSet<ToolCallId>,
    /// Exact nodes examined by rebuilds and incremental folds in tests.
    #[cfg(test)]
    nodes_examined: usize,
    /// Exact set insertions and removals performed in tests.
    #[cfg(test)]
    mutations: usize,
}

impl LoadedToolCallIds {
    /// Bounds global identity-index growth for this event's new assistant node.
    pub(super) fn append_allocation_charge(&self, event: &tau_proto::Event) -> usize {
        let tau_proto::Event::ProviderResponseFinished(response) = event else {
            return 0;
        };
        let mut count = 0usize;
        let mut bytes = 0usize;
        for item in &response.output_items {
            if let ContextItem::ToolCall(call) = item {
                count = count.saturating_add(1);
                bytes = bytes.saturating_add(call.call_id.as_str().len());
            }
        }
        bytes.saturating_add(hash_growth::<ToolCallId>(
            self.ids.len(),
            self.ids.capacity(),
            count,
        ))
    }

    /// Adds identities from nodes appended by one successful canonical fold.
    pub(super) fn extend_nodes(&mut self, nodes: &[AgentNode]) {
        #[cfg(test)]
        {
            self.nodes_examined = self.nodes_examined.saturating_add(nodes.len());
        }
        for node in nodes {
            let AgentEntry::AssistantResponse { output_items, .. } = &node.entry else {
                continue;
            };
            for call_id in output_items.iter().filter_map(|item| match item {
                ContextItem::ToolCall(call) => Some(call.call_id.clone()),
                _ => None,
            }) {
                #[cfg(test)]
                {
                    if self.ids.insert(call_id) {
                        self.mutations = self.mutations.saturating_add(1);
                    }
                }
                #[cfg(not(test))]
                self.ids.insert(call_id);
            }
        }
    }

    /// Rebuilds the index after a loaded-tree set is replaced or cleared.
    pub(super) fn rebuild<'a>(&mut self, trees: impl IntoIterator<Item = &'a AgentTree>) {
        #[cfg(test)]
        {
            self.mutations = self.mutations.saturating_add(self.ids.len());
        }
        self.ids.clear();
        for tree in trees {
            self.extend_nodes(tree.nodes());
        }
    }

    /// Returns the exact identities currently present in loaded trees.
    pub(super) fn ids(&self) -> &HashSet<ToolCallId> {
        &self.ids
    }

    /// Returns exact mutation and node-work counters for focused tests.
    #[cfg(test)]
    pub(super) fn counters(&self) -> (usize, usize) {
        (self.mutations, self.nodes_examined)
    }
}
