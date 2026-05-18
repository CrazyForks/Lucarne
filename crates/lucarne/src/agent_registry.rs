use std::sync::Arc;

use linkme::distributed_slice;

use crate::{adapter::ProtocolAdapter, ProviderId};

pub(crate) type AdapterFactory = fn() -> Arc<ProtocolAdapter>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentDescriptor {
    pub(crate) id: ProviderId,
    pub(crate) order: u16,
    pub(crate) adapter_factory: Option<AdapterFactory>,
}

#[distributed_slice]
pub(crate) static ALL_AGENT_DESCRIPTORS: [AgentDescriptor];

pub(crate) fn all_agent_descriptors() -> Vec<&'static AgentDescriptor> {
    let mut descriptors: Vec<_> = ALL_AGENT_DESCRIPTORS.iter().collect();
    descriptors.sort_by_key(|descriptor| (descriptor.order, descriptor.id));
    descriptors
}

pub(crate) fn adapter_descriptors() -> Vec<&'static AgentDescriptor> {
    all_agent_descriptors()
        .into_iter()
        .filter(|descriptor| descriptor.adapter_factory.is_some())
        .collect()
}
