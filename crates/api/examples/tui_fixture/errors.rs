use mediaops_home_client::ClientError;

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("usage: tui_fixture DIR [rich|empty|not-ready]")]
    Usage,
    #[error("unknown mode `{0}`")]
    UnknownMode(String),
    #[error("scratch directory must be an absolute dedicated path")]
    ScratchNotDedicated,
    #[error("invalid fixture input: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Api(#[from] ClientError),
}
