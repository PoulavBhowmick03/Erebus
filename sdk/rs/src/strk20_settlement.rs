//! Checked boundary for legacy STRK20 settlement.
//!
//! Legacy requests retain their offer ids, journal, and wire formats. They are not canonical
//! v1 agreements: STRK20 advertises no canonical suite and cannot accept their authorizations.

use erebus_core::settlement::{check_requirements, BackendCapabilities};
use erebus_core::terms::{Guarantee, GuaranteeSet, SettlementMode};

use crate::capabilities::strk20_capabilities;
use crate::client::{Client, ClientError, OfferId, SettlementReceipt};
use crate::operation::OperationId;
use crate::state::ChannelHandle;

/// Requirements checked before any legacy settlement work or journal mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacySettlementRequirements {
    /// Settlement mechanism the caller permits.
    pub mode: SettlementMode,
    /// Guarantees the caller requires; unsupported guarantees are never removed.
    pub guarantees: GuaranteeSet,
    /// Refuse hosted proving when true.
    pub require_local_proving: bool,
}

impl Default for LegacySettlementRequirements {
    fn default() -> Self {
        let mut guarantees = GuaranteeSet::from_guarantee(Guarantee::HiddenAmount);
        guarantees.insert(Guarantee::HiddenRecipient);
        Self {
            mode: SettlementMode::Shielded,
            guarantees,
            require_local_proving: false,
        }
    }
}

/// Adapter for the existing STRK20 execution path. No canonical suite translation occurs.
pub struct Strk20SettlementBackend<'a> {
    client: &'a Client,
}

impl<'a> Strk20SettlementBackend<'a> {
    /// Uses the client's existing chain, pool, state, and execution configuration.
    #[must_use]
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Returns guarantees common to all supported legacy wire versions.
    #[must_use]
    pub fn capabilities(&self) -> BackendCapabilities {
        strk20_capabilities()
    }

    /// Checks requirements, then executes the existing journaled settlement operation.
    pub async fn settle(
        &self,
        requirements: LegacySettlementRequirements,
        operation_id: &OperationId,
        handle: ChannelHandle,
        offer_id: OfferId,
    ) -> Result<SettlementReceipt, ClientError> {
        check_requirements(
            requirements.mode,
            requirements.guarantees,
            requirements.require_local_proving,
            &self.capabilities(),
        )?;
        self.client
            .settle_strk20(operation_id, handle, offer_id)
            .await
    }
}
