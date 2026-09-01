//! Job state machines (AD-10). Pure: no sqlite, no tokio, no filesystem.
//!
//! [`advance`] is the only legal state transition. Readiness predicates such as
//! [`encode_ready`] live here so `sync` and `encode` cannot each invent one.

use std::fmt;

use crate::title_id::TitleId;

/// Stable row id. Assigned by the jobs repository; never zero or negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(i64);

impl JobId {
    pub fn new(raw: i64) -> Result<Self, JobError> {
        if raw <= 0 {
            return Err(JobError::InvalidId(raw));
        }
        Ok(Self(raw))
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Long-running operation kind. Match every variant; do not add a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    Want,
    Pull,
    Encode,
    Hold,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Want => "want",
            Self::Pull => "pull",
            Self::Encode => "encode",
            Self::Hold => "hold",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, JobError> {
        match raw {
            "want" => Ok(Self::Want),
            "pull" => Ok(Self::Pull),
            "encode" => Ok(Self::Encode),
            "hold" => Ok(Self::Hold),
            other => Err(JobError::UnknownKind(other.to_string())),
        }
    }

    /// Kind the parent row must have when a parent is set. Encode may omit a
    /// parent (already-local file). Pull may omit one. Want and Hold must not
    /// have one. When Encode does set a parent it must be a Pull.
    pub fn required_parent_kind(self) -> Option<JobKind> {
        match self {
            Self::Pull => Some(Self::Want),
            Self::Encode => Some(Self::Pull),
            Self::Want | Self::Hold => None,
        }
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Wanted-title lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WantState {
    Open,
    Satisfied,
    Dropped,
}

impl WantState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Satisfied => "satisfied",
            Self::Dropped => "dropped",
        }
    }
}

/// Copy/pull lifecycle. Terminal [`Installed`] is what makes an Encode ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullState {
    Queued,
    Pulling,
    Verifying,
    Installed,
}

impl PullState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Pulling => "pulling",
            Self::Verifying => "verifying",
            Self::Installed => "installed",
        }
    }
}

/// Home encode lifecycle. Starts [`Queued`] until the parent Copy is Installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncodeState {
    Queued,
    Encoding,
    Replacing,
    Done,
}

impl EncodeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Encoding => "encoding",
            Self::Replacing => "replacing",
            Self::Done => "done",
        }
    }
}

/// Holds-inbox row. Operator-driven; not timer-advanced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldState {
    Open,
    Approved,
    Rejected,
}

impl HoldState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// Tagged per-kind state. [`JobKind`] is derived from the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobState {
    Want(WantState),
    Pull(PullState),
    Encode(EncodeState),
    Hold(HoldState),
}

impl JobState {
    pub fn initial(kind: JobKind) -> Self {
        match kind {
            JobKind::Want => Self::Want(WantState::Open),
            JobKind::Pull => Self::Pull(PullState::Queued),
            JobKind::Encode => Self::Encode(EncodeState::Queued),
            JobKind::Hold => Self::Hold(HoldState::Open),
        }
    }

    pub fn kind(self) -> JobKind {
        match self {
            Self::Want(_) => JobKind::Want,
            Self::Pull(_) => JobKind::Pull,
            Self::Encode(_) => JobKind::Encode,
            Self::Hold(_) => JobKind::Hold,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Want(s) => s.as_str(),
            Self::Pull(s) => s.as_str(),
            Self::Encode(s) => s.as_str(),
            Self::Hold(s) => s.as_str(),
        }
    }

    pub fn parse(kind: JobKind, raw: &str) -> Result<Self, JobError> {
        match kind {
            JobKind::Want => match raw {
                "open" => Ok(Self::Want(WantState::Open)),
                "satisfied" => Ok(Self::Want(WantState::Satisfied)),
                "dropped" => Ok(Self::Want(WantState::Dropped)),
                other => Err(JobError::UnknownState {
                    kind,
                    state: other.to_string(),
                }),
            },
            JobKind::Pull => match raw {
                "queued" => Ok(Self::Pull(PullState::Queued)),
                "pulling" => Ok(Self::Pull(PullState::Pulling)),
                "verifying" => Ok(Self::Pull(PullState::Verifying)),
                "installed" => Ok(Self::Pull(PullState::Installed)),
                other => Err(JobError::UnknownState {
                    kind,
                    state: other.to_string(),
                }),
            },
            JobKind::Encode => match raw {
                "queued" => Ok(Self::Encode(EncodeState::Queued)),
                "encoding" => Ok(Self::Encode(EncodeState::Encoding)),
                "replacing" => Ok(Self::Encode(EncodeState::Replacing)),
                "done" => Ok(Self::Encode(EncodeState::Done)),
                other => Err(JobError::UnknownState {
                    kind,
                    state: other.to_string(),
                }),
            },
            JobKind::Hold => match raw {
                "open" => Ok(Self::Hold(HoldState::Open)),
                "approved" => Ok(Self::Hold(HoldState::Approved)),
                "rejected" => Ok(Self::Hold(HoldState::Rejected)),
                other => Err(JobError::UnknownState {
                    kind,
                    state: other.to_string(),
                }),
            },
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind().as_str(), self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WantEvent {
    Satisfy,
    Drop,
}

impl WantEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Satisfy => "satisfy",
            Self::Drop => "drop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullEvent {
    Start,
    FinishRanges,
    Install,
}

impl PullEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::FinishRanges => "finish_ranges",
            Self::Install => "install",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncodeEvent {
    Start,
    FinishEncode,
    Replace,
}

impl EncodeEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::FinishEncode => "finish_encode",
            Self::Replace => "replace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldEvent {
    Approve,
    Reject,
}

impl HoldEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

/// Input to [`advance`]. Kind must match the state's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobEvent {
    Want(WantEvent),
    Pull(PullEvent),
    Encode(EncodeEvent),
    Hold(HoldEvent),
}

impl JobEvent {
    pub fn kind(self) -> JobKind {
        match self {
            Self::Want(_) => JobKind::Want,
            Self::Pull(_) => JobKind::Pull,
            Self::Encode(_) => JobKind::Encode,
            Self::Hold(_) => JobKind::Hold,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Want(e) => e.as_str(),
            Self::Pull(e) => e.as_str(),
            Self::Encode(e) => e.as_str(),
            Self::Hold(e) => e.as_str(),
        }
    }
}

impl fmt::Display for JobEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind().as_str(), self.as_str())
    }
}

/// One jobs row. [`title_id`] is the subject. [`parent_job_id`] links an action
/// job to the job it waits on (a Copy/Pull job for Encode; the originating
/// want for Pull).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    id: JobId,
    title_id: TitleId,
    state: JobState,
    parent_job_id: Option<JobId>,
}

impl Job {
    pub fn new(
        id: JobId,
        title_id: TitleId,
        state: JobState,
        parent_job_id: Option<JobId>,
    ) -> Result<Self, JobError> {
        if parent_job_id == Some(id) {
            return Err(JobError::SelfParent(id));
        }
        match state.kind() {
            JobKind::Want | JobKind::Hold if parent_job_id.is_some() => {
                return Err(JobError::UnexpectedParent { kind: state.kind() });
            }
            JobKind::Want | JobKind::Hold | JobKind::Pull | JobKind::Encode => {}
        }
        Ok(Self {
            id,
            title_id,
            state,
            parent_job_id,
        })
    }

    pub fn id(&self) -> JobId {
        self.id
    }

    pub fn title_id(&self) -> &TitleId {
        &self.title_id
    }

    pub fn kind(&self) -> JobKind {
        self.state.kind()
    }

    pub fn state(&self) -> JobState {
        self.state
    }

    pub fn parent_job_id(&self) -> Option<JobId> {
        self.parent_job_id
    }

    /// Apply [`advance`] and keep id / title / parent.
    pub fn advance(&self, event: JobEvent) -> Result<Self, JobError> {
        Ok(Self {
            id: self.id,
            title_id: self.title_id.clone(),
            state: crate::jobs::advance(&self.state, event)?,
            parent_job_id: self.parent_job_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("job id must be a positive integer, got {0}")]
    InvalidId(i64),
    #[error("job {0} cannot be its own parent")]
    SelfParent(JobId),
    #[error("encode job requires a parent pull job")]
    EncodeNeedsParent,
    #[error("{kind} job cannot have a parent")]
    UnexpectedParent { kind: JobKind },
    #[error("parent job {parent} is {actual}, expected {expected}")]
    ParentKindMismatch {
        parent: JobId,
        expected: JobKind,
        actual: JobKind,
    },
    #[error("unknown job kind `{0}`")]
    UnknownKind(String),
    #[error("unknown job state `{state}` for kind {kind}")]
    UnknownState { kind: JobKind, state: String },
    #[error("job event kind {event} does not match state kind {state}")]
    KindMismatch { state: JobKind, event: JobKind },
    #[error("illegal {kind} transition from {from} via {event}")]
    IllegalTransition {
        kind: JobKind,
        from: &'static str,
        event: &'static str,
    },
}

/// Pure transition. An illegal step is an error, not a caller convention.
pub fn advance(state: &JobState, event: JobEvent) -> Result<JobState, JobError> {
    match state {
        JobState::Want(s) => match event {
            JobEvent::Want(e) => advance_want(*s, e),
            JobEvent::Pull(_) | JobEvent::Encode(_) | JobEvent::Hold(_) => {
                Err(JobError::KindMismatch {
                    state: JobKind::Want,
                    event: event.kind(),
                })
            }
        },
        JobState::Pull(s) => match event {
            JobEvent::Pull(e) => advance_pull(*s, e),
            JobEvent::Want(_) | JobEvent::Encode(_) | JobEvent::Hold(_) => {
                Err(JobError::KindMismatch {
                    state: JobKind::Pull,
                    event: event.kind(),
                })
            }
        },
        JobState::Encode(s) => match event {
            JobEvent::Encode(e) => advance_encode(*s, e),
            JobEvent::Want(_) | JobEvent::Pull(_) | JobEvent::Hold(_) => {
                Err(JobError::KindMismatch {
                    state: JobKind::Encode,
                    event: event.kind(),
                })
            }
        },
        JobState::Hold(s) => match event {
            JobEvent::Hold(e) => advance_hold(*s, e),
            JobEvent::Want(_) | JobEvent::Pull(_) | JobEvent::Encode(_) => {
                Err(JobError::KindMismatch {
                    state: JobKind::Hold,
                    event: event.kind(),
                })
            }
        },
    }
}

fn advance_want(state: WantState, event: WantEvent) -> Result<JobState, JobError> {
    match (state, event) {
        (WantState::Open, WantEvent::Satisfy) => Ok(JobState::Want(WantState::Satisfied)),
        (WantState::Open, WantEvent::Drop) => Ok(JobState::Want(WantState::Dropped)),
        (WantState::Satisfied, WantEvent::Satisfy)
        | (WantState::Satisfied, WantEvent::Drop)
        | (WantState::Dropped, WantEvent::Satisfy)
        | (WantState::Dropped, WantEvent::Drop) => {
            Err(illegal(JobKind::Want, state.as_str(), event.as_str()))
        }
    }
}

fn advance_pull(state: PullState, event: PullEvent) -> Result<JobState, JobError> {
    match (state, event) {
        (PullState::Queued, PullEvent::Start) => Ok(JobState::Pull(PullState::Pulling)),
        (PullState::Pulling, PullEvent::FinishRanges) => Ok(JobState::Pull(PullState::Verifying)),
        (PullState::Verifying, PullEvent::Install) => Ok(JobState::Pull(PullState::Installed)),
        (PullState::Queued, PullEvent::FinishRanges)
        | (PullState::Queued, PullEvent::Install)
        | (PullState::Pulling, PullEvent::Start)
        | (PullState::Pulling, PullEvent::Install)
        | (PullState::Verifying, PullEvent::Start)
        | (PullState::Verifying, PullEvent::FinishRanges)
        | (PullState::Installed, PullEvent::Start)
        | (PullState::Installed, PullEvent::FinishRanges)
        | (PullState::Installed, PullEvent::Install) => {
            Err(illegal(JobKind::Pull, state.as_str(), event.as_str()))
        }
    }
}

fn advance_encode(state: EncodeState, event: EncodeEvent) -> Result<JobState, JobError> {
    match (state, event) {
        (EncodeState::Queued, EncodeEvent::Start) => Ok(JobState::Encode(EncodeState::Encoding)),
        (EncodeState::Encoding, EncodeEvent::FinishEncode) => {
            Ok(JobState::Encode(EncodeState::Replacing))
        }
        (EncodeState::Replacing, EncodeEvent::Replace) => Ok(JobState::Encode(EncodeState::Done)),
        (EncodeState::Queued, EncodeEvent::FinishEncode)
        | (EncodeState::Queued, EncodeEvent::Replace)
        | (EncodeState::Encoding, EncodeEvent::Start)
        | (EncodeState::Encoding, EncodeEvent::Replace)
        | (EncodeState::Replacing, EncodeEvent::Start)
        | (EncodeState::Replacing, EncodeEvent::FinishEncode)
        | (EncodeState::Done, EncodeEvent::Start)
        | (EncodeState::Done, EncodeEvent::FinishEncode)
        | (EncodeState::Done, EncodeEvent::Replace) => {
            Err(illegal(JobKind::Encode, state.as_str(), event.as_str()))
        }
    }
}

fn advance_hold(state: HoldState, event: HoldEvent) -> Result<JobState, JobError> {
    match (state, event) {
        (HoldState::Open, HoldEvent::Approve) => Ok(JobState::Hold(HoldState::Approved)),
        (HoldState::Open, HoldEvent::Reject) => Ok(JobState::Hold(HoldState::Rejected)),
        (HoldState::Approved, HoldEvent::Approve)
        | (HoldState::Approved, HoldEvent::Reject)
        | (HoldState::Rejected, HoldEvent::Approve)
        | (HoldState::Rejected, HoldEvent::Reject) => {
            Err(illegal(JobKind::Hold, state.as_str(), event.as_str()))
        }
    }
}

fn illegal(kind: JobKind, from: &'static str, event: &'static str) -> JobError {
    JobError::IllegalTransition { kind, from, event }
}

/// Encode is ready when it is still queued and either its parent Pull is
/// Installed, or it has no parent and the title already has a title-index row.
pub fn encode_ready(job: &Job, parent: Option<&Job>, title_indexed: bool) -> bool {
    match job.state() {
        JobState::Encode(EncodeState::Queued) => {}
        JobState::Encode(EncodeState::Encoding)
        | JobState::Encode(EncodeState::Replacing)
        | JobState::Encode(EncodeState::Done)
        | JobState::Want(_)
        | JobState::Pull(_)
        | JobState::Hold(_) => return false,
    }
    match job.parent_job_id() {
        Some(parent_id) => {
            let Some(parent) = parent else {
                return false;
            };
            parent.id() == parent_id
                && matches!(parent.state(), JobState::Pull(PullState::Installed))
        }
        None => parent.is_none() && title_indexed,
    }
}

/// Jobs repository port (AD-8, AD-10). `advance` is the sole state write.
///
/// A trait, not I/O: async signatures only. The adapter lives in `store`.
#[allow(async_fn_in_trait)]
pub trait JobsRepo: Send + Sync {
    type Error;

    async fn get(&self, id: JobId) -> Result<Option<Job>, Self::Error>;
    async fn list(&self) -> Result<Vec<Job>, Self::Error>;
    async fn list_by_title(&self, title_id: &TitleId) -> Result<Vec<Job>, Self::Error>;
    async fn create(
        &self,
        kind: JobKind,
        title_id: &TitleId,
        parent_job_id: Option<JobId>,
    ) -> Result<Job, Self::Error>;
    async fn advance(&self, id: JobId, event: JobEvent) -> Result<Job, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: i64) -> JobId {
        JobId::new(n).expect("id")
    }

    fn title() -> TitleId {
        TitleId::movie("603").expect("title")
    }

    fn job(n: i64, state: JobState, parent: Option<i64>) -> Job {
        Job::new(id(n), title(), state, parent.map(id)).expect("job")
    }

    #[test]
    fn job_kind_match_is_exhaustive() {
        let kinds = [JobKind::Want, JobKind::Pull, JobKind::Encode, JobKind::Hold];
        for kind in kinds {
            let token = match kind {
                JobKind::Want => "want",
                JobKind::Pull => "pull",
                JobKind::Encode => "encode",
                JobKind::Hold => "hold",
            };
            assert_eq!(kind.as_str(), token);
            assert_eq!(JobKind::parse(token).expect("parse"), kind);
            assert_eq!(JobState::initial(kind).kind(), kind);
        }
        assert_eq!(kinds.len(), 4);
    }

    #[test]
    fn pull_advances_queued_to_installed() {
        let mut state = JobState::initial(JobKind::Pull);
        state = advance(&state, JobEvent::Pull(PullEvent::Start)).expect("start");
        assert_eq!(state, JobState::Pull(PullState::Pulling));
        state = advance(&state, JobEvent::Pull(PullEvent::FinishRanges)).expect("ranges");
        assert_eq!(state, JobState::Pull(PullState::Verifying));
        state = advance(&state, JobEvent::Pull(PullEvent::Install)).expect("install");
        assert_eq!(state, JobState::Pull(PullState::Installed));
    }

    #[test]
    fn pull_skips_and_regressions_are_illegal() {
        let queued = JobState::Pull(PullState::Queued);
        let err = advance(&queued, JobEvent::Pull(PullEvent::Install)).expect_err("skip");
        assert!(
            matches!(
                err,
                JobError::IllegalTransition {
                    kind: JobKind::Pull,
                    ..
                }
            ),
            "{err}"
        );
        let installed = JobState::Pull(PullState::Installed);
        let err = advance(&installed, JobEvent::Pull(PullEvent::Start)).expect_err("terminal");
        assert!(matches!(err, JobError::IllegalTransition { .. }), "{err}");
    }

    #[test]
    fn kind_mismatch_is_an_error() {
        let state = JobState::Pull(PullState::Queued);
        let err = advance(&state, JobEvent::Encode(EncodeEvent::Start)).expect_err("mismatch");
        assert_eq!(
            err,
            JobError::KindMismatch {
                state: JobKind::Pull,
                event: JobKind::Encode,
            }
        );
    }

    #[test]
    fn encode_ready_when_parent_copy_is_installed() {
        let parent = job(1, JobState::Pull(PullState::Installed), None);
        let encode = job(2, JobState::Encode(EncodeState::Queued), Some(1));
        assert!(encode_ready(&encode, Some(&parent), false));
    }

    #[test]
    fn encode_ready_without_parent_when_title_is_indexed() {
        let encode = job(2, JobState::Encode(EncodeState::Queued), None);
        assert!(encode_ready(&encode, None, true));
        assert!(!encode_ready(&encode, None, false));
    }

    #[test]
    fn encode_is_not_ready_until_parent_copy_is_installed() {
        let parent = job(1, JobState::Pull(PullState::Verifying), None);
        let encode = job(2, JobState::Encode(EncodeState::Queued), Some(1));
        assert!(!encode_ready(&encode, Some(&parent), true));
        assert!(!encode_ready(&encode, None, true));
        let want = job(1, JobState::Want(WantState::Satisfied), None);
        assert!(!encode_ready(&encode, Some(&want), true));
        let wrong_parent = job(9, JobState::Pull(PullState::Installed), None);
        assert!(!encode_ready(&encode, Some(&wrong_parent), true));
        let started = job(2, JobState::Encode(EncodeState::Encoding), Some(1));
        let installed = job(1, JobState::Pull(PullState::Installed), None);
        assert!(!encode_ready(&started, Some(&installed), true));
        let pull = job(3, JobState::Pull(PullState::Queued), Some(1));
        assert!(!encode_ready(&pull, Some(&installed), true));
    }

    #[test]
    fn parent_job_id_is_on_the_type() {
        let child = job(2, JobState::Encode(EncodeState::Queued), Some(1));
        assert_eq!(child.parent_job_id(), Some(id(1)));
        assert_eq!(child.kind(), JobKind::Encode);
        assert_eq!(child.title_id(), &title());
        let root = job(1, JobState::Want(WantState::Open), None);
        assert_eq!(root.parent_job_id(), None);
        assert_eq!(root.title_id(), &title());
    }

    #[test]
    fn job_cannot_parent_itself() {
        let err = Job::new(
            id(1),
            title(),
            JobState::Pull(PullState::Queued),
            Some(id(1)),
        )
        .expect_err("self parent");
        assert!(matches!(err, JobError::SelfParent(_)), "{err}");
    }

    #[test]
    fn encode_parent_is_optional_and_want_forbids_one() {
        let local = Job::new(id(1), title(), JobState::Encode(EncodeState::Queued), None)
            .expect("already-local encode");
        assert_eq!(local.parent_job_id(), None);
        let err = Job::new(id(1), title(), JobState::Want(WantState::Open), Some(id(2)))
            .expect_err("want parent");
        assert!(
            matches!(
                err,
                JobError::UnexpectedParent {
                    kind: JobKind::Want
                }
            ),
            "{err}"
        );
        let err = Job::new(id(1), title(), JobState::Hold(HoldState::Open), Some(id(2)))
            .expect_err("hold parent");
        assert!(
            matches!(
                err,
                JobError::UnexpectedParent {
                    kind: JobKind::Hold
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn want_and_hold_and_encode_happy_paths() {
        let want = advance(
            &JobState::Want(WantState::Open),
            JobEvent::Want(WantEvent::Satisfy),
        )
        .expect("satisfy");
        assert_eq!(want, JobState::Want(WantState::Satisfied));
        let dropped = advance(
            &JobState::Want(WantState::Open),
            JobEvent::Want(WantEvent::Drop),
        )
        .expect("drop");
        assert_eq!(dropped, JobState::Want(WantState::Dropped));

        let mut enc = JobState::Encode(EncodeState::Queued);
        enc = advance(&enc, JobEvent::Encode(EncodeEvent::Start)).expect("enc start");
        enc = advance(&enc, JobEvent::Encode(EncodeEvent::FinishEncode)).expect("encoded");
        enc = advance(&enc, JobEvent::Encode(EncodeEvent::Replace)).expect("replace");
        assert_eq!(enc, JobState::Encode(EncodeState::Done));

        let approved = advance(
            &JobState::Hold(HoldState::Open),
            JobEvent::Hold(HoldEvent::Approve),
        )
        .expect("approve");
        assert_eq!(approved, JobState::Hold(HoldState::Approved));
        let rejected = advance(
            &JobState::Hold(HoldState::Open),
            JobEvent::Hold(HoldEvent::Reject),
        )
        .expect("reject");
        assert_eq!(rejected, JobState::Hold(HoldState::Rejected));
        let err = advance(&approved, JobEvent::Hold(HoldEvent::Reject)).expect_err("terminal hold");
        assert!(matches!(
            err,
            JobError::IllegalTransition {
                kind: JobKind::Hold,
                ..
            }
        ));
    }

    #[test]
    fn job_id_rejects_zero_and_negative() {
        assert!(matches!(JobId::new(0), Err(JobError::InvalidId(0))));
        assert!(matches!(JobId::new(-1), Err(JobError::InvalidId(-1))));
        assert_eq!(id(1).get(), 1);
    }

    #[test]
    fn state_round_trip_tokens() {
        for kind in [JobKind::Want, JobKind::Pull, JobKind::Encode, JobKind::Hold] {
            let state = JobState::initial(kind);
            assert_eq!(JobState::parse(kind, state.as_str()).expect("parse"), state);
        }
        let installed = JobState::Pull(PullState::Installed);
        assert_eq!(
            JobState::parse(JobKind::Pull, "installed").expect("installed"),
            installed
        );
        assert!(JobState::parse(JobKind::Pull, "done").is_err());
        assert!(JobKind::parse("copy").is_err());
    }
}
