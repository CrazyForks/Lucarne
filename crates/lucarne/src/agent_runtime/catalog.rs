use crate::ProviderId;
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownAgentProvider {
    pub id: ProviderId,
    pub display_name: SmolStr,
    pub runtime_label: SmolStr,
    pub binary: SmolStr,
}
