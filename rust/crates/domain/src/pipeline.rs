#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    Initialized,
    ParsedJobConfig,
    SettingsLoaded,
    TempPrepared,
    Delegating,
    Finished,
    Failed,
    Cancelled,
}

impl PipelineState {
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Initialized, Self::ParsedJobConfig)
                | (Self::ParsedJobConfig, Self::SettingsLoaded)
                | (Self::SettingsLoaded, Self::TempPrepared)
                | (Self::TempPrepared, Self::Delegating)
                | (Self::Delegating, Self::Finished)
                | (Self::Delegating, Self::Cancelled)
                | (Self::Delegating, Self::Failed)
                | (_, Self::Failed)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::PipelineState;

    #[test]
    fn valid_transitions_succeed() {
        assert!(PipelineState::Initialized.can_transition_to(PipelineState::ParsedJobConfig));
        assert!(PipelineState::ParsedJobConfig.can_transition_to(PipelineState::SettingsLoaded));
        assert!(PipelineState::SettingsLoaded.can_transition_to(PipelineState::TempPrepared));
        assert!(PipelineState::TempPrepared.can_transition_to(PipelineState::Delegating));
        assert!(PipelineState::Delegating.can_transition_to(PipelineState::Finished));
        assert!(PipelineState::Delegating.can_transition_to(PipelineState::Cancelled));
    }

    #[test]
    fn invalid_transition_is_rejected() {
        assert!(!PipelineState::Initialized.can_transition_to(PipelineState::Delegating));
    }
}
