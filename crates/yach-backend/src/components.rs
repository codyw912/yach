//! Reference components and the presets that select them.
//! Design: docs/project/specs/2026-09-27-distribution-presets-design.md.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Component {
    ProjectTools,
    BaselineGuidance,
    Hashline,
    JevReviewer,
    SkillIndex,
}

impl Component {
    pub const ALL: [Component; 5] = [
        Self::ProjectTools,
        Self::BaselineGuidance,
        Self::Hashline,
        Self::JevReviewer,
        Self::SkillIndex,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ProjectTools => "project-tools",
            Self::BaselineGuidance => "baseline-guidance",
            Self::Hashline => "hashline",
            Self::JevReviewer => "jev-reviewer",
            Self::SkillIndex => "skill-index",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|component| component.name() == name)
    }

    #[must_use]
    pub const fn bundled_extension_id(self) -> Option<&'static str> {
        match self {
            Self::Hashline => Some("yach.hashline"),
            Self::JevReviewer => Some("yach.jev-reviewer"),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_kernel(self) -> bool {
        self.bundled_extension_id().is_none()
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Minimal,
    Full,
}

impl Preset {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Full => "full",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Minimal, Self::Full]
            .into_iter()
            .find(|preset| preset.name() == name)
    }

    #[must_use]
    pub const fn components(self) -> &'static [Component] {
        match self {
            Self::Minimal => &[Component::SkillIndex],
            Self::Full => &Component::ALL,
        }
    }
}

/// Enabled components for one session. `bash` is kernel and always present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentSet {
    bits: u8,
}

impl ComponentSet {
    #[must_use]
    pub const fn full() -> Self {
        Self { bits: 0b1_1111 }
    }

    #[must_use]
    pub fn from_preset(preset: Preset) -> Self {
        preset
            .components()
            .iter()
            .fold(Self { bits: 0 }, |set, component| {
                set.with(*component, true)
            })
    }

    #[must_use]
    pub const fn contains(self, component: Component) -> bool {
        self.bits & component.bit() != 0
    }

    #[must_use]
    pub const fn with(self, component: Component, enabled: bool) -> Self {
        let bits = if enabled {
            self.bits | component.bit()
        } else {
            self.bits & !component.bit()
        };
        Self { bits }
    }

    #[must_use]
    pub const fn project_tools(self) -> bool {
        self.contains(Component::ProjectTools)
    }

    #[must_use]
    pub const fn baseline_guidance(self) -> bool {
        self.contains(Component::BaselineGuidance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_membership_matches_the_spec() {
        let minimal = ComponentSet::from_preset(Preset::Minimal);
        assert!(!minimal.project_tools());
        assert!(!minimal.baseline_guidance());
        assert!(minimal.contains(Component::SkillIndex));
        assert!(!minimal.contains(Component::Hashline));

        let full = ComponentSet::from_preset(Preset::Full);
        for component in Component::ALL {
            assert!(full.contains(component), "{}", component.name());
        }
        assert_eq!(full, ComponentSet::full());
    }

    #[test]
    fn names_round_trip_and_reject_unknown() {
        for component in Component::ALL {
            assert_eq!(Component::parse(component.name()), Some(component));
        }
        assert_eq!(Component::parse("profile"), None);
        assert_eq!(Preset::parse("minimal"), Some(Preset::Minimal));
        assert_eq!(Preset::parse("full"), Some(Preset::Full));
        assert_eq!(Preset::parse("default"), None);
    }

    #[test]
    fn only_extension_components_map_to_bundled_ids() {
        assert_eq!(
            Component::Hashline.bundled_extension_id(),
            Some("yach.hashline")
        );
        assert_eq!(
            Component::JevReviewer.bundled_extension_id(),
            Some("yach.jev-reviewer")
        );
        assert_eq!(Component::ProjectTools.bundled_extension_id(), None);
        assert!(Component::BaselineGuidance.is_kernel());
        assert!(!Component::Hashline.is_kernel());
    }
}
