//! Fixed hazard signal set for reviewer assessments.

use crate::ActionClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReviewSignal {
    Install,
    Activation,
    Publish,
    Disclosure,
    Delete,
    IrreversibleLoss,
    Privilege,
    RemoteCode,
    OpaqueEffect,
    OriginConfusion,
    ScopeConflict,
}

impl ReviewSignal {
    pub const ALL: [Self; 11] = [
        Self::Install,
        Self::Activation,
        Self::Publish,
        Self::Disclosure,
        Self::Delete,
        Self::IrreversibleLoss,
        Self::Privilege,
        Self::RemoteCode,
        Self::OpaqueEffect,
        Self::OriginConfusion,
        Self::ScopeConflict,
    ];
    /// Hold as SignificantRisk when at threshold. `Delete` is absent by design.
    pub const RISK: [Self; 8] = [
        Self::Install,
        Self::Activation,
        Self::Publish,
        Self::Disclosure,
        Self::IrreversibleLoss,
        Self::Privilege,
        Self::RemoteCode,
        Self::OriginConfusion,
    ];
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Activation => "activation",
            Self::Publish => "publish",
            Self::Disclosure => "disclosure",
            Self::Delete => "delete",
            Self::IrreversibleLoss => "irreversible_loss",
            Self::Privilege => "privilege",
            Self::RemoteCode => "remote_code",
            Self::OpaqueEffect => "opaque_effect",
            Self::OriginConfusion => "origin_confusion",
            Self::ScopeConflict => "scope_conflict",
        }
    }
    #[must_use]
    pub const fn policy_class(self) -> Option<ActionClass> {
        match self {
            Self::Install => Some(ActionClass::PersistentInstall),
            Self::Activation => Some(ActionClass::HostActivation),
            Self::Publish => Some(ActionClass::ExternalPublish),
            Self::Disclosure => Some(ActionClass::SensitiveDisclosure),
            Self::Delete => Some(ActionClass::DestructiveDelete),
            _ => None,
        }
    }
}
