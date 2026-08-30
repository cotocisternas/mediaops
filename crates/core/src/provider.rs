//! Provider kinds. v1 ships SwizzinBox + AlreadyThere; every other variant is loud.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    SwizzinBox,
    AlreadyThere,
    DockerCompose,
    #[serde(rename = "ultra.cc")]
    UltraCc,
    QuickBox,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SwizzinBox => "swizzin_box",
            Self::AlreadyThere => "already_there",
            Self::DockerCompose => "docker_compose",
            Self::UltraCc => "ultra.cc",
            Self::QuickBox => "quickbox",
        }
    }

    pub fn parse(name: &str) -> Result<Self, ProviderError> {
        match name {
            "swizzin_box" | "swizzin-box" | "SwizzinBox" => Ok(Self::SwizzinBox),
            "already_there" | "already-there" | "AlreadyThere" => Ok(Self::AlreadyThere),
            "docker_compose" | "docker-compose" | "DockerCompose" => Ok(Self::DockerCompose),
            "ultra.cc" | "ultra-cc" | "Ultra.cc" => Ok(Self::UltraCc),
            "quickbox" | "QuickBox" => Ok(Self::QuickBox),
            other => Err(ProviderError::Unknown(other.to_string())),
        }
    }

    /// AlreadyThere is a no-op install. SwizzinBox is implemented in `ssh`.
    pub fn ensure_installable(self) -> Result<(), ProviderError> {
        match self {
            Self::SwizzinBox | Self::AlreadyThere => Ok(()),
            other => Err(ProviderError::Unimplemented(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("unknown provider `{0}`")]
    Unknown(String),
    #[error("provider `{0}` is unimplemented")]
    Unimplemented(ProviderKind),
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// AlreadyThere: no-op install, configure via APIs (AD-21).
pub fn already_there_install() -> Result<(), ProviderError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_providers_fail_loudly() {
        for kind in [
            ProviderKind::DockerCompose,
            ProviderKind::UltraCc,
            ProviderKind::QuickBox,
        ] {
            assert_eq!(
                kind.ensure_installable(),
                Err(ProviderError::Unimplemented(kind))
            );
        }
    }

    #[test]
    fn v1_providers_are_installable() {
        assert_eq!(ProviderKind::AlreadyThere.ensure_installable(), Ok(()));
        assert_eq!(ProviderKind::SwizzinBox.ensure_installable(), Ok(()));
        assert_eq!(already_there_install(), Ok(()));
    }

    #[test]
    fn unimplemented_is_never_ok() {
        assert!(!matches!(
            ProviderKind::DockerCompose.ensure_installable(),
            Ok(())
        ));
    }
}
