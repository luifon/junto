//! The update check (ADR 0704): whether the source of releases offers a newer build than
//! the one that runs. Nothing is downloaded and nothing is installed here. Without a source
//! no request is made.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::flow::machine::lock;
use crate::runtime::Runtime;

/// The time between two checks that the client makes by itself.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// A source that has not answered after this time has failed.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest address of a release page, in bytes.
pub const LONGEST_PAGE: usize = 512;

const LONGEST_VERSION: usize = 32;

/// The version of a build of the client: three numbers, compared from the first one on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a version is three numbers with a dot between them, such as 0.3.1")]
pub struct InvalidVersion;

impl Version {
    /// The version of the package that this build was made from.
    pub const THIS_BUILD: Self = Self::new(
        number(env!("JUNTO_BUILD_MAJOR")),
        number(env!("JUNTO_BUILD_MINOR")),
        number(env!("JUNTO_BUILD_PATCH")),
    );

    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Reads a version from text such as `0.3.1`. Each number is written in decimal digits
    /// without a sign and without a leading zero.
    pub fn parse(text: &str) -> Result<Self, InvalidVersion> {
        if text.len() > LONGEST_VERSION {
            return Err(InvalidVersion);
        }
        let mut parts = text.split('.');
        let mut next = || parts.next().and_then(part).ok_or(InvalidVersion);
        let version = Self::new(next()?, next()?, next()?);
        match parts.next() {
            None => Ok(version),
            Some(_) => Err(InvalidVersion),
        }
    }

    pub const fn major(self) -> u32 {
        self.major
    }

    pub const fn minor(self) -> u32 {
        self.minor
    }

    pub const fn patch(self) -> u32 {
        self.patch
    }
}

fn part(text: &str) -> Option<u32> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let plain = digits && (text == "0" || !text.starts_with('0'));
    if plain { text.parse().ok() } else { None }
}

/// One number of the version of the package.
const fn number(text: &str) -> u32 {
    let digits = text.as_bytes();
    assert!(!digits.is_empty(), "the package version has three numbers");
    let mut value: u32 = 0;
    let mut index = 0;
    while index < digits.len() {
        assert!(
            digits[index].is_ascii_digit(),
            "the package version has three numbers"
        );
        value = value * 10 + (digits[index] - b'0') as u32;
        index += 1;
    }
    value
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = InvalidVersion;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// The address of the page of a release. It is text of the source: an `https` address of
/// at most [`LONGEST_PAGE`] bytes with a host, without a user name or a password, and
/// without a control character or a space. It is held in the form that a URL parser
/// writes, which has ASCII characters only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReleasePage(String);

impl ReleasePage {
    /// `None` when the text is not such an address.
    pub fn new(text: &str) -> Option<Self> {
        let plain = |text: &str| {
            text.len() <= LONGEST_PAGE
                && !text
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
        };
        if !plain(text) {
            return None;
        }
        // The parser of addresses leaves a tab and a line end out, so the text is checked
        // before it.
        let address = url::Url::parse(text).ok()?;
        let shown = address.scheme() == "https"
            && address.host_str().is_some_and(|host| !host.is_empty())
            && address.username().is_empty()
            && address.password().is_none()
            && plain(address.as_str());
        shown.then(|| Self(address.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReleasePage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A build that the source of releases offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Where a person reads about the release. Absent when the source named no page, or one
    /// that is not a [`ReleasePage`].
    pub page: Option<ReleasePage>,
}

impl Release {
    /// A release as a source states it. A page that is not a [`ReleasePage`] is left out.
    pub fn new(version: Version, page: Option<&str>) -> Self {
        Self {
            version,
            page: page.and_then(ReleasePage::new),
        }
    }
}

/// Why a source gave no release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error("the source of releases did not answer")]
    Unreachable,
    #[error("the answer of the source of releases cannot be used")]
    Invalid,
    #[error("the release information has expired")]
    Expired,
    #[error("this build trusts none of the release signing keys")]
    UntrustedKeys,
}

pub type NewestFuture = Pin<Box<dyn Future<Output = Result<Release, UpdateError>> + Send>>;

/// Where the client learns which build is the newest (ADR 0704). A check calls it once.
pub trait UpdateSource: Send + Sync + 'static {
    /// The newest release that the source offers.
    fn newest(&self) -> NewestFuture;
}

/// What the client knows about a newer build.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    /// No check has ended. Without a source this is the state for ever.
    #[default]
    NotChecked,
    /// The newest release is not newer than this build.
    Current,
    Available(Release),
    /// No check has succeeded. A check that fails after one that succeeded leaves the
    /// answer of that one.
    Failed,
    Expired,
    UntrustedKeys,
}

struct Shared {
    build: Version,
    source: Option<Arc<dyn UpdateSource>>,
    runtime: Runtime,
    state: watch::Sender<UpdateState>,
    /// Held while a check asks the source, so that one request runs at a time.
    asking: Arc<tokio::sync::Mutex<()>>,
    /// The number of checks that have ended.
    ended: AtomicU64,
    /// The task that checks every [`CHECK_INTERVAL`].
    periodic: Mutex<Option<AbortHandle>>,
    /// The version of the newest release in the last answer that the client trusts.
    newest: Mutex<Option<Version>>,
    /// Called after each change of the state and of the newest version.
    told: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn state(&self) -> UpdateState {
        self.state.borrow().clone()
    }

    /// Takes the answer of one check into the state.
    fn record(&self, answer: Result<Release, UpdateError>) -> UpdateState {
        let newest = match &answer {
            Ok(release) => Some(Some(release.version)),
            Err(UpdateError::Expired | UpdateError::UntrustedKeys) => Some(None),
            Err(UpdateError::Unreachable | UpdateError::Invalid) => None,
        };
        let newest_changed = newest.is_some_and(|newest| {
            let mut held = lock(&self.newest);
            let changed = *held != newest;
            *held = newest;
            changed
        });
        let found = match answer {
            Ok(release) if release.version > self.build => Some(UpdateState::Available(release)),
            Ok(_) => Some(UpdateState::Current),
            Err(UpdateError::Expired) => Some(UpdateState::Expired),
            Err(UpdateError::UntrustedKeys) => {
                tracing::warn!(
                    "the update information is not signed by a key that this build trusts; this build does not update itself"
                );
                Some(UpdateState::UntrustedKeys)
            }
            Err(error) => {
                tracing::debug!(%error, "the update check failed");
                None
            }
        };
        let changed = self.state.send_if_modified(|state| {
            let next = match found {
                Some(found) => found,
                None if *state == UpdateState::NotChecked => UpdateState::Failed,
                None => return false,
            };
            let changed = *state != next;
            *state = next;
            changed
        });
        self.ended.fetch_add(1, Ordering::AcqRel);
        if changed || newest_changed {
            (self.told)();
        }
        self.state()
    }
}

/// The update check of one client. The tasks that it starts hold no reference to it, and
/// its periodic check ends when it is dropped.
pub struct UpdateCheck {
    shared: Arc<Shared>,
}

impl UpdateCheck {
    /// `build`: the version that runs. `source`: where the newest release is read; without
    /// one nothing is ever asked.
    pub fn new(build: Version, source: Option<Arc<dyn UpdateSource>>, runtime: Runtime) -> Self {
        Self::with_notice(build, source, runtime, || {})
    }

    /// Like [`UpdateCheck::new`]. `told` is called after each change of the state.
    pub(crate) fn with_notice(
        build: Version,
        source: Option<Arc<dyn UpdateSource>>,
        runtime: Runtime,
        told: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                build,
                source,
                runtime,
                state: watch::Sender::new(UpdateState::NotChecked),
                asking: Arc::new(tokio::sync::Mutex::new(())),
                ended: AtomicU64::new(0),
                periodic: Mutex::new(None),
                newest: Mutex::new(None),
                told: Box::new(told),
            }),
        }
    }

    /// The version of the build that runs.
    pub fn build(&self) -> Version {
        self.shared.build
    }

    /// False when no check can ever be made.
    pub fn has_source(&self) -> bool {
        self.shared.source.is_some()
    }

    pub fn state(&self) -> UpdateState {
        self.shared.state()
    }

    /// The version of the newest release that the source named in the last answer that
    /// the client trusts, also when it is not newer than this build. The server and the
    /// client of one release have one version (ADR 0780). An answer that has expired or
    /// that no trusted key signed leaves none; a check that fails otherwise keeps the
    /// version of the check before it. Without a source it is always absent.
    pub fn newest(&self) -> Option<Version> {
        *lock(&self.shared.newest)
    }

    /// A value that gives the state after each change.
    pub fn watch(&self) -> UpdateWatch {
        UpdateWatch {
            receiver: self.shared.state.subscribe(),
        }
    }

    /// Asks the source once and returns the state after the answer. The check runs as a
    /// task of the client runtime and to its end when the returned future is dropped. A
    /// call while a check runs waits for that check and asks nothing by itself. Without a
    /// source nothing is asked.
    pub fn check(&self) -> impl Future<Output = UpdateState> + Send + 'static + use<> {
        let before = self.state();
        let work = self.has_source().then(|| {
            self.shared
                .runtime
                .run(check_once(Arc::downgrade(&self.shared)))
        });
        async move {
            match work {
                Some(work) => work.await.ok().flatten().unwrap_or(before),
                None => before,
            }
        }
    }

    /// Starts the periodic check: one check now and one every [`CHECK_INTERVAL`] after the
    /// end of the one before. A second call changes nothing. Without a source nothing
    /// starts.
    pub fn start(&self) {
        if !self.has_source() {
            return;
        }
        let mut periodic = lock(&self.shared.periodic);
        if periodic.is_none() {
            let task = repeat(Arc::downgrade(&self.shared));
            let task = self.shared.runtime.handle().spawn(task);
            *periodic = Some(task.abort_handle());
        }
    }
}

impl Drop for UpdateCheck {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.shared.periodic).take() {
            task.abort();
        }
    }
}

/// One check. `None` when the update check does not exist any more or has no source.
async fn check_once(shared: Weak<Shared>) -> Option<UpdateState> {
    let (source, asking, ended) = {
        let shared = shared.upgrade()?;
        let ended = shared.ended.load(Ordering::Acquire);
        (shared.source.clone()?, shared.asking.clone(), ended)
    };
    let _asking = asking.lock_owned().await;
    {
        let shared = shared.upgrade()?;
        // A check that ended while this one waited is the answer of this one too.
        if shared.ended.load(Ordering::Acquire) != ended {
            return Some(shared.state());
        }
    }
    let answer = tokio::time::timeout(CHECK_TIMEOUT, source.newest())
        .await
        .unwrap_or(Err(UpdateError::Unreachable));
    Some(shared.upgrade()?.record(answer))
}

async fn repeat(shared: Weak<Shared>) {
    while check_once(shared.clone()).await.is_some() {
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

/// Wakes a reader when the state of the update check has changed.
pub struct UpdateWatch {
    receiver: watch::Receiver<UpdateState>,
}

impl UpdateWatch {
    /// Waits for a change and returns the newest state. `None` when the update check does
    /// not exist any more.
    pub async fn changed(&mut self) -> Option<UpdateState> {
        self.receiver.changed().await.ok()?;
        Some(self.receiver.borrow_and_update().clone())
    }

    /// Waits until the state fulfils a condition and returns it.
    pub async fn wait_for(
        &mut self,
        holds: impl FnMut(&UpdateState) -> bool,
    ) -> Option<UpdateState> {
        self.receiver
            .wait_for(holds)
            .await
            .ok()
            .map(|state| state.clone())
    }
}

/// A source for tests. It answers every check with the answer that it holds and counts the
/// checks. It makes no request.
pub struct FakeSource {
    answer: Mutex<Result<Release, UpdateError>>,
    checks: watch::Sender<usize>,
}

impl FakeSource {
    pub fn new(answer: Result<Release, UpdateError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            checks: watch::Sender::new(0),
        })
    }

    /// The answer of every check from now on.
    pub fn set(&self, answer: Result<Release, UpdateError>) {
        *lock(&self.answer) = answer;
    }

    /// How often the source was asked.
    pub fn checks(&self) -> usize {
        *self.checks.borrow()
    }

    /// Waits until the source was asked `checks` times.
    pub async fn wait_for_checks(&self, checks: usize) {
        let mut asked = self.checks.subscribe();
        let _ = asked.wait_for(|asked| *asked >= checks).await;
    }
}

impl UpdateSource for FakeSource {
    fn newest(&self) -> NewestFuture {
        self.checks.send_modify(|checks| *checks += 1);
        let answer = lock(&self.answer).clone();
        Box::pin(async move { answer })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use tokio::time::Instant;

    use super::*;

    const BUILD: Version = Version::new(0, 3, 1);

    fn release(text: &str) -> Release {
        let page = format!("https://junto.example.test/releases/{text}");
        Release::new(Version::parse(text).unwrap(), Some(&page))
    }

    fn checker(source: &Arc<FakeSource>) -> UpdateCheck {
        let runtime = Runtime::current_or_new().unwrap();
        UpdateCheck::new(BUILD, Some(source.clone()), runtime)
    }

    /// Lets the tasks of the runtime run until none of them can go on.
    async fn settle() {
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
    }

    fn alive_tasks() -> usize {
        tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
    }

    #[test]
    fn a_version_is_three_numbers_and_is_read_from_text() {
        assert_eq!(Version::parse("0.3.1"), Ok(Version::new(0, 3, 1)));
        assert_eq!("10.20.30".parse::<Version>(), Ok(Version::new(10, 20, 30)));
        let largest = Version::new(u32::MAX, u32::MAX, u32::MAX);
        assert_eq!(
            Version::parse("4294967295.4294967295.4294967295"),
            Ok(largest)
        );
        let version = Version::new(1, 0, 12);
        assert_eq!(
            (version.major(), version.minor(), version.patch()),
            (1, 0, 12)
        );
        assert_eq!(version.to_string(), "1.0.12");
        assert_eq!(Version::parse(&version.to_string()), Ok(version));

        let huge = "1.".repeat(1 << 20);
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "v1.2.3",
            " 1.2.3",
            "1.2.3 ",
            "1.2.3\n",
            "1..3",
            ".1.2",
            "1.2.",
            "-1.2.3",
            "+1.2.3",
            "01.2.3",
            "1.02.3",
            "1.2.00",
            "1.2.3-beta",
            "1.2.3+build",
            "1,2,3",
            "4294967296.0.0",
            "0.0.99999999999999999999",
            "١.٢.٣",
            huge.as_str(),
        ] {
            assert_eq!(Version::parse(text), Err(InvalidVersion), "{text:.40}");
        }
    }

    #[test]
    fn versions_are_ordered_by_their_numbers_from_the_first_on() {
        let ordered = [
            "0.0.9", "0.3.1", "0.3.2", "0.3.10", "0.4.0", "0.10.0", "1.0.0",
        ];
        let versions: Vec<Version> = ordered
            .iter()
            .map(|text| Version::parse(text).unwrap())
            .collect();
        for (index, lower) in versions.iter().enumerate() {
            for higher in &versions[index + 1..] {
                assert!(lower < higher, "{lower} {higher}");
                assert!(higher > lower, "{lower} {higher}");
            }
            assert_eq!(lower.cmp(lower), std::cmp::Ordering::Equal);
        }
        assert_eq!(versions.iter().max(), versions.last());
    }

    #[test]
    fn the_version_of_this_build_is_the_version_of_the_package() {
        assert_eq!(Version::THIS_BUILD.to_string(), env!("CARGO_PKG_VERSION"));
        assert_eq!(
            Version::parse(env!("CARGO_PKG_VERSION")),
            Ok(Version::THIS_BUILD)
        );
        assert_eq!(number("0"), 0);
        assert_eq!(number("4294967295"), u32::MAX);
    }

    #[test]
    fn a_page_is_kept_only_when_it_is_a_short_https_address() {
        let page = ReleasePage::new("https://junto.example.test/releases/0.4.0").unwrap();
        assert_eq!(page.as_str(), "https://junto.example.test/releases/0.4.0");
        assert_eq!(page.to_string(), page.as_str());
        let written = ReleasePage::new("HTTPS://Junto.Example.Test").unwrap();
        assert_eq!(written.as_str(), "https://junto.example.test/");

        let start = "https://junto.example.test/";
        let longest = format!("{start}{}", "a".repeat(LONGEST_PAGE - start.len()));
        assert_eq!(ReleasePage::new(&longest).unwrap().as_str().len(), 512);
        let too_long = format!("{longest}a");
        // Each letter is two bytes in the text and six in the address that is held.
        let grows = format!("{start}{}", "é".repeat(200));
        assert!(grows.len() <= LONGEST_PAGE);
        let huge = format!("{start}{}", "a".repeat(8 << 20));

        for text in [
            "",
            "junto.example.test/releases",
            "/releases/0.4.0",
            "//junto.example.test/releases",
            "http://junto.example.test/releases",
            "ftp://junto.example.test/releases",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,release",
            "https:",
            "https://",
            "https://ada@junto.example.test/",
            "https://ada:secret@junto.example.test/",
            "https://junto.example.test/a b",
            " https://junto.example.test/",
            "https://junto.example.test/\n",
            "https://junto.example.test/\u{7}",
            "https://junto.example.test/\u{1b}[2J",
            "https://junto.exa\tmple.test/",
            "https://junto.example.test/\u{85}",
            too_long.as_str(),
            grows.as_str(),
            huge.as_str(),
        ] {
            assert_eq!(ReleasePage::new(text), None, "{text:.60}");
        }

        // Text outside ASCII is held in the written form of an address.
        let named = ReleasePage::new("https://jüntö.example.test/vérsion").unwrap();
        assert!(named.as_str().is_ascii(), "{named}");
        assert!(named.as_str().starts_with("https://xn--"), "{named}");
        let hidden = ReleasePage::new("https://junto.example.test/\u{202e}txt.exe").unwrap();
        assert!(hidden.as_str().is_ascii(), "{hidden}");

        let version = Version::new(0, 4, 0);
        assert_eq!(
            Release::new(version, Some("http://junto.example.test/")),
            Release {
                version,
                page: None
            }
        );
        assert_eq!(Release::new(version, Some(&huge)).page, None);
        assert_eq!(Release::new(version, None).page, None);
        assert_eq!(
            Release::new(version, Some("https://junto.example.test/r")).page,
            ReleasePage::new("https://junto.example.test/r")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_source_nothing_is_asked_and_the_state_stays_not_checked() {
        let runtime = Runtime::current_or_new().unwrap();
        let check = UpdateCheck::new(BUILD, None, runtime);
        assert!(!check.has_source());
        assert_eq!(check.build(), BUILD);
        assert_eq!(check.state(), UpdateState::NotChecked);
        assert_eq!(check.check().await, UpdateState::NotChecked);
        check.start();
        settle().await;
        assert_eq!(alive_tasks(), 0);
        tokio::time::advance(CHECK_INTERVAL * 3).await;
        settle().await;
        assert_eq!(check.state(), UpdateState::NotChecked);
    }

    #[tokio::test(start_paused = true)]
    async fn a_newer_release_is_available_and_an_equal_or_an_older_one_is_current() {
        let source = FakeSource::new(Ok(release("0.3.1")));
        let check = checker(&source);
        assert!(check.has_source());
        assert_eq!(check.state(), UpdateState::NotChecked);
        assert_eq!(source.checks(), 0);

        assert_eq!(check.check().await, UpdateState::Current);
        assert_eq!(check.state(), UpdateState::Current);
        assert_eq!(source.checks(), 1);
        for older in ["0.3.0", "0.2.9", "0.0.0"] {
            source.set(Ok(release(older)));
            assert_eq!(check.check().await, UpdateState::Current, "{older}");
        }

        for newer in ["0.3.2", "0.4.0", "1.0.0"] {
            source.set(Ok(release(newer)));
            let available = UpdateState::Available(release(newer));
            assert_eq!(check.check().await, available, "{newer}");
            assert_eq!(check.state(), available);
        }
        let UpdateState::Available(found) = check.state() else {
            panic!("no release");
        };
        assert_eq!(found.version, Version::new(1, 0, 0));
        assert_eq!(
            found.page.unwrap().as_str(),
            "https://junto.example.test/releases/1.0.0"
        );

        // The source offers the running build again: nothing is available any more.
        source.set(Ok(release("0.3.1")));
        assert_eq!(check.check().await, UpdateState::Current);
        assert_eq!(source.checks(), 8);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_check_keeps_the_earlier_answer() {
        let source = FakeSource::new(Err(UpdateError::Unreachable));
        let check = checker(&source);
        assert_eq!(check.check().await, UpdateState::Failed);
        source.set(Err(UpdateError::Invalid));
        assert_eq!(check.check().await, UpdateState::Failed);

        source.set(Ok(release("0.3.1")));
        assert_eq!(check.check().await, UpdateState::Current);
        for error in [UpdateError::Unreachable, UpdateError::Invalid] {
            source.set(Err(error));
            assert_eq!(check.check().await, UpdateState::Current, "{error}");
        }

        source.set(Ok(release("0.4.0")));
        let available = UpdateState::Available(release("0.4.0"));
        assert_eq!(check.check().await, available);
        for error in [UpdateError::Unreachable, UpdateError::Invalid] {
            source.set(Err(error));
            assert_eq!(check.check().await, available, "{error}");
            assert_eq!(check.state(), available);
        }
        assert_eq!(source.checks(), 8);
    }

    #[tokio::test(start_paused = true)]
    async fn the_newest_version_follows_each_trusted_answer_and_is_told() {
        let source = FakeSource::new(Ok(release("0.3.0")));
        let runtime = Runtime::current_or_new().unwrap();
        let told = Arc::new(AtomicUsize::new(0));
        let counted = told.clone();
        let check = UpdateCheck::with_notice(BUILD, Some(source.clone()), runtime, move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(check.newest(), None);
        assert_eq!(check.check().await, UpdateState::Current);
        assert_eq!(check.newest(), Some(Version::new(0, 3, 0)));
        assert_eq!(told.load(Ordering::SeqCst), 1);
        // Another release that is not newer than the build leaves the state, and its
        // version is told.
        source.set(Ok(release("0.3.1")));
        assert_eq!(check.check().await, UpdateState::Current);
        assert_eq!(check.newest(), Some(BUILD));
        assert_eq!(told.load(Ordering::SeqCst), 2);
        assert_eq!(check.check().await, UpdateState::Current);
        assert_eq!(told.load(Ordering::SeqCst), 2);

        source.set(Ok(release("0.4.0")));
        check.check().await;
        assert_eq!(check.newest(), Some(Version::new(0, 4, 0)));
        for error in [UpdateError::Unreachable, UpdateError::Invalid] {
            source.set(Err(error));
            check.check().await;
            assert_eq!(check.newest(), Some(Version::new(0, 4, 0)), "{error}");
        }
        for error in [UpdateError::Expired, UpdateError::UntrustedKeys] {
            source.set(Ok(release("0.4.0")));
            check.check().await;
            source.set(Err(error));
            check.check().await;
            assert_eq!(check.newest(), None, "{error}");
        }
    }

    /// A source whose answer waits until the test lets it go.
    struct Held {
        release: watch::Sender<bool>,
        checks: AtomicUsize,
    }

    impl UpdateSource for Held {
        fn newest(&self) -> NewestFuture {
            self.checks.fetch_add(1, Ordering::SeqCst);
            let mut released = self.release.subscribe();
            Box::pin(async move {
                let _ = released.wait_for(|released| *released).await;
                Ok(release("0.4.0"))
            })
        }
    }

    fn held() -> Arc<Held> {
        Arc::new(Held {
            release: watch::Sender::new(false),
            checks: AtomicUsize::new(0),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_source_that_does_not_answer_fails_after_the_time_of_a_check() {
        let source = held();
        let runtime = Runtime::current_or_new().unwrap();
        let check = UpdateCheck::new(BUILD, Some(source.clone()), runtime);
        let started = Instant::now();
        assert_eq!(check.check().await, UpdateState::Failed);
        assert_eq!(started.elapsed(), CHECK_TIMEOUT);
        assert_eq!(source.checks.load(Ordering::SeqCst), 1);
        settle().await;
        assert_eq!(alive_tasks(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn checks_at_the_same_time_ask_the_source_once() {
        let source = held();
        let runtime = Runtime::current_or_new().unwrap();
        let check = UpdateCheck::new(BUILD, Some(source.clone()), runtime);
        let (first, second, third) = (check.check(), check.check(), check.check());
        settle().await;
        assert_eq!(source.checks.load(Ordering::SeqCst), 1);
        assert_eq!(check.state(), UpdateState::NotChecked);

        source.release.send_replace(true);
        let available = UpdateState::Available(release("0.4.0"));
        assert_eq!(first.await, available);
        assert_eq!(second.await, available);
        assert_eq!(third.await, available);
        assert_eq!(source.checks.load(Ordering::SeqCst), 1);

        // A check that starts after the end of another one asks again.
        assert_eq!(check.check().await, available);
        assert_eq!(source.checks.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_runs_to_its_end_when_the_caller_drops_the_future() {
        let source = FakeSource::new(Ok(release("0.4.0")));
        let check = checker(&source);
        let mut watch = check.watch();
        drop(check.check());
        let state = watch.changed().await;
        assert_eq!(state, Some(UpdateState::Available(release("0.4.0"))));
    }

    #[tokio::test(start_paused = true)]
    async fn each_change_of_the_state_is_told_once() {
        let source = FakeSource::new(Ok(release("0.3.1")));
        let told = Arc::new(AtomicUsize::new(0));
        let counted = told.clone();
        let check = UpdateCheck::with_notice(
            BUILD,
            Some(source.clone()),
            Runtime::current_or_new().unwrap(),
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
            },
        );
        let mut watch = check.watch();
        let told = || told.load(Ordering::SeqCst);

        check.check().await;
        assert_eq!(told(), 1);
        assert_eq!(watch.changed().await, Some(UpdateState::Current));
        // The same answer and a failure change nothing.
        check.check().await;
        source.set(Err(UpdateError::Unreachable));
        check.check().await;
        assert_eq!(told(), 1);

        source.set(Ok(release("0.4.0")));
        check.check().await;
        assert_eq!(told(), 2);
        let available = UpdateState::Available(release("0.4.0"));
        assert_eq!(watch.changed().await, Some(available.clone()));
        check.check().await;
        assert_eq!(told(), 2);
        source.set(Ok(release("0.5.0")));
        check.check().await;
        assert_eq!(told(), 3);
        let found = watch.wait_for(|state| *state != available).await.unwrap();
        assert_eq!(found, UpdateState::Available(release("0.5.0")));

        drop(check);
        assert_eq!(watch.changed().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_client_checks_at_the_start_and_every_six_hours() {
        let source = FakeSource::new(Ok(release("0.4.0")));
        let check = checker(&source);
        let mut watch = check.watch();
        let started = Instant::now();
        check.start();
        check.start();
        let available = UpdateState::Available(release("0.4.0"));
        assert_eq!(watch.changed().await, Some(available.clone()));
        assert_eq!(started.elapsed(), Duration::ZERO);
        settle().await;
        assert_eq!(source.checks(), 1);
        assert_eq!(alive_tasks(), 1);

        tokio::time::advance(CHECK_INTERVAL - Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(source.checks(), 1);

        source.set(Ok(release("0.5.0")));
        source.wait_for_checks(2).await;
        assert_eq!(started.elapsed(), CHECK_INTERVAL);
        let newer = UpdateState::Available(release("0.5.0"));
        assert_eq!(watch.changed().await, Some(newer.clone()));

        // A failing source is asked again after the same time, and the answer stays.
        source.set(Err(UpdateError::Unreachable));
        source.wait_for_checks(3).await;
        assert_eq!(started.elapsed(), CHECK_INTERVAL * 2);
        source.wait_for_checks(4).await;
        assert_eq!(started.elapsed(), CHECK_INTERVAL * 3);
        settle().await;
        assert_eq!(check.state(), newer);
        assert_eq!(source.checks(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn the_periodic_check_ends_with_the_update_check() {
        let source = FakeSource::new(Ok(release("0.4.0")));
        let check = checker(&source);
        check.start();
        source.wait_for_checks(1).await;
        settle().await;
        assert_eq!(alive_tasks(), 1);

        drop(check);
        settle().await;
        assert_eq!(alive_tasks(), 0);
        assert_eq!(Arc::strong_count(&source), 1);
        tokio::time::advance(CHECK_INTERVAL * 3).await;
        settle().await;
        assert_eq!(source.checks(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_in_flight_does_not_keep_a_dropped_update_check() {
        let source = held();
        let runtime = Runtime::current_or_new().unwrap();
        let check = UpdateCheck::new(BUILD, Some(source.clone()), runtime);
        check.start();
        let asked = check.check();
        settle().await;
        assert_eq!(source.checks.load(Ordering::SeqCst), 1);
        let mut watch = check.watch();

        drop(check);
        assert_eq!(watch.changed().await, None);
        source.release.send_replace(true);
        assert_eq!(asked.await, UpdateState::NotChecked);
        settle().await;
        assert_eq!(alive_tasks(), 0);
        assert_eq!(source.checks.load(Ordering::SeqCst), 1);
    }
}
