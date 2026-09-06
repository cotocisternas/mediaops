use std::path::{Path, PathBuf};

use super::errors::FixtureError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Rich,
    Empty,
    NotReady,
}

impl Mode {
    pub fn parse(raw: &str) -> Result<Self, FixtureError> {
        match raw {
            "rich" => Ok(Self::Rich),
            "empty" => Ok(Self::Empty),
            "not-ready" => Ok(Self::NotReady),
            other => Err(FixtureError::UnknownMode(other.to_string())),
        }
    }

    pub const fn heartbeats(self) -> bool {
        match self {
            Self::Rich | Self::Empty => true,
            Self::NotReady => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub dir: PathBuf,
    pub mode: Mode,
}

pub fn parse_launch<I, S>(args: I) -> Result<Launch, FixtureError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let dir = PathBuf::from(args.next().ok_or(FixtureError::Usage)?.as_ref());
    let mode = match args.next() {
        Some(raw) => Mode::parse(raw.as_ref())?,
        None => Mode::Rich,
    };
    if args.next().is_some() {
        return Err(FixtureError::Usage);
    }
    if !is_absolute_scratch_candidate(&dir) {
        return Err(FixtureError::ScratchNotDedicated);
    }
    Ok(Launch { dir, mode })
}

fn is_absolute_scratch_candidate(dir: &Path) -> bool {
    dir.is_absolute()
        && dir.parent().is_some_and(|parent| parent != Path::new("/"))
        && !dir.as_os_str().is_empty()
}
