//! How this copy of Junto updates, the update action, the status of its background work,
//! and the restart after an update (ADR 0780, ADR 0726).
use super::AppState;
use crate::text::book as txt;
use gpui::Context;
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

/// The work of one update: on success the sentence that states the result.
type Work = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
type Install = Arc<dyn Fn() -> Work + Send + Sync>;

/// How this copy of Junto gets a newer version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateWay {
    /// The update action installs it over this copy, and a restart starts it.
    InApp,
    /// The location of this copy cannot be written, or macOS runs it from a translocated
    /// copy: the person moves it first.
    MoveFirst,
    /// The Debian package: the system package manager installs the newer `.deb`.
    SystemPackage,
    /// A copy that no package installed, such as a build of the source.
    NotAPackage,
    /// A build without an update source: it does not look for newer versions.
    NoSource,
}

#[derive(Default)]
pub(super) struct InstallState {
    action: Option<Install>,
    way: Option<UpdateWay>,
    /// The download page of the release site, when the source is its `updates/` directory.
    site: Option<String>,
    working: bool,
    installed: bool,
    status: Option<String>,
}

impl InstallState {
    pub fn new() -> Self {
        Self::with_source(None)
    }

    /// The update of this copy from the compiled source, or from `base` in a test
    /// configuration (`--update-source`).
    pub fn with_source(base: Option<String>) -> Self {
        let mut state = Self::default();
        let compiled = junto_client_core::releases::HttpSource::compiled();
        let source = match &base {
            Some(base) => junto_client_core::releases::HttpSource::new(
                base,
                match junto_client_core::releases::TrustedKeys::compiled() {
                    Ok(keys) => keys,
                    Err(_) => {
                        state.way = Some(UpdateWay::NoSource);
                        return state;
                    }
                },
                junto_client_core::updates::Version::THIS_BUILD,
                junto_client_core::releases::Target::this_build(
                    junto_client_core::releases::install::Package::running(),
                ),
            )
            .ok(),
            None => compiled.ok().flatten(),
        };
        let Some(source) = source else {
            state.way = Some(UpdateWay::NoSource);
            return state;
        };
        state.site = source.site().map(String::from);
        if junto_client_core::releases::install::running_from_system_package() {
            state.way = Some(UpdateWay::SystemPackage);
            return state;
        }
        match junto_client_core::releases::PlatformInstaller::running() {
            Ok(installer) if installer.location().is_ok() => {
                state.way = Some(UpdateWay::InApp);
                state.action = Some(Arc::new(move || {
                    let base = base.clone();
                    Box::pin(async move {
                        junto_client_core::releases::update_running_from(base.as_deref())
                            .await
                            .map(|installed| txt::installed(&installed.previous))
                            .map_err(|error| error.to_string())
                    })
                }));
            }
            Ok(_) => {
                state.way = Some(UpdateWay::MoveFirst);
                state.status = Some(txt::MOVE_TO_UPDATE.into());
            }
            Err(_) => {
                state.way = Some(UpdateWay::NotAPackage);
                state.status = Some(txt::NOT_A_PACKAGE.into());
            }
        }
        state
    }
}

/// What the button of the version line does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionAction {
    /// Downloads and installs the newer version over this copy.
    Update,
    /// Quits and starts the installed version.
    Restart,
    /// Opens the section "Version" of the settings, which holds the steps.
    Details,
}

/// The version of this copy below "Add server": the version, a sentence of where the
/// update stands, and the button of the next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionLine {
    pub version: String,
    pub sentence: String,
    pub action: Option<VersionAction>,
    /// A newer version is available or installed: the line is drawn as a notice.
    pub newer: bool,
}

/// The version line of a state of the update check and of the update of this copy.
/// `failure` is the sentence of an update that failed.
pub fn version_line(
    view: &super::book::VersionView,
    way: &UpdateWay,
    installing: bool,
    installed: bool,
    failure: Option<&str>,
) -> VersionLine {
    use super::book::CheckShown;
    let version = txt::client_version(view.build);
    let (sentence, action, newer) = match &view.check {
        CheckShown::Available { version, .. } if installed => (
            txt::restart_to_use(*version),
            Some(VersionAction::Restart),
            true,
        ),
        CheckShown::Available { .. } if installing => (txt::INSTALLING.to_owned(), None, true),
        CheckShown::Available { version, .. } => match way {
            UpdateWay::InApp => (
                failure.map_or_else(|| txt::update_available(*version), str::to_owned),
                Some(VersionAction::Update),
                true,
            ),
            other => (
                txt::available_way(*version, other),
                Some(VersionAction::Details),
                true,
            ),
        },
        CheckShown::NotConfigured => (txt::NO_SOURCE.to_owned(), None, false),
        CheckShown::NotChecked if view.checking => (txt::LOOKING.to_owned(), None, false),
        CheckShown::NotChecked => (txt::NOT_LOOKED.to_owned(), None, false),
        CheckShown::Current => (txt::UP_TO_DATE.to_owned(), None, false),
        CheckShown::Failed | CheckShown::Expired | CheckShown::UntrustedKeys => (
            txt::NOT_FOUND_OUT.to_owned(),
            Some(VersionAction::Details),
            false,
        ),
    };
    VersionLine {
        version,
        sentence,
        action,
        newer,
    }
}

/// The program that a restart starts: the AppImage file that the person started, not the
/// program inside its mount, which ends with the process. Other packages start the same
/// path again, which the update replaced in place.
pub fn restart_path(appimage: Option<std::ffi::OsString>) -> Option<PathBuf> {
    appimage.filter(|path| !path.is_empty()).map(PathBuf::from)
}

impl AppState {
    pub fn supports_update_install(&self) -> bool {
        self.update_install.action.is_some()
    }
    pub fn can_install_update(&self) -> bool {
        self.supports_update_install()
            && self.available_update().is_some()
            && !self.update_install.working
            && !self.update_install.installed
    }
    pub fn update_install_status(&self) -> Option<&str> {
        self.update_install.status.as_deref()
    }

    /// How this copy gets a newer version. Absent before the start read it.
    pub fn update_way(&self) -> UpdateWay {
        self.update_install
            .way
            .clone()
            .unwrap_or(UpdateWay::NoSource)
    }

    /// The download page of the release site of this build.
    pub fn release_site(&self) -> Option<&str> {
        self.update_install.site.as_deref()
    }

    /// An update is being downloaded and installed.
    pub fn update_installing(&self) -> bool {
        self.update_install.working
    }

    /// An update was installed: a restart starts the new version.
    pub fn update_installed(&self) -> bool {
        self.update_install.installed
    }

    /// The version line below "Add server".
    pub fn version_line(&self) -> VersionLine {
        let install = &self.update_install;
        let failure = (!install.working && !install.installed)
            .then(|| install.status.as_deref())
            .flatten()
            .filter(|_| install.action.is_some());
        version_line(
            &self.version_view(),
            &self.update_way(),
            install.working,
            install.installed,
            failure,
        )
    }

    /// The step of the version line: the update, the restart, or the section "Version".
    pub fn act_on_version_line(&mut self, cx: &mut Context<Self>) -> Option<VersionAction> {
        let action = self.version_line().action?;
        match action {
            VersionAction::Update => self.install_update(cx),
            VersionAction::Restart => self.restart_after_update(cx),
            VersionAction::Details => {}
        }
        Some(action)
    }

    /// Uses the update of this copy from another source. Only a test configuration
    /// accepts one (`--update-source`).
    pub fn use_update_source(&mut self, base: Option<String>) {
        self.update_install = InstallState::with_source(base);
    }

    #[cfg(feature = "test-support")]
    pub fn set_update_installer(&mut self, action: impl Fn() -> Work + Send + Sync + 'static) {
        self.update_install = InstallState {
            action: Some(Arc::new(action)),
            way: Some(UpdateWay::InApp),
            ..InstallState::default()
        };
    }

    /// Draws this way of the update and this download page. The screen tool and the tests
    /// of the window use it.
    #[cfg(feature = "test-support")]
    pub fn show_update_way_fixture(
        &mut self,
        way: UpdateWay,
        site: Option<&str>,
        installed: bool,
        cx: &mut Context<Self>,
    ) {
        self.update_install.way = Some(way);
        self.update_install.site = site.map(str::to_owned);
        self.update_install.installed = installed;
        if installed {
            self.update_install.status = Some(txt::installed(
                &junto_client_core::releases::install::Previous::Kept(
                    junto_client_core::updates::Version::THIS_BUILD,
                ),
            ));
        }
        cx.notify();
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        if !self.can_install_update() {
            return;
        }
        let Some(action) = self.update_install.action.clone() else {
            return;
        };
        self.update_install.working = true;
        self.update_install.status = Some(txt::INSTALLING.into());
        let work = self.client.runtime().handle().spawn(action());
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.update_install.working = false;
                this.update_install.installed = matches!(result, Ok(Ok(_)));
                this.update_install.status = Some(match result {
                    Ok(Ok(sentence)) => sentence,
                    Ok(Err(error)) => txt::install_failed(&error),
                    Err(_) => txt::INSTALL_STOPPED.into(),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Quits and starts the installed version, after an update. The call is left first, as
    /// at every quit.
    pub fn restart_after_update(&mut self, cx: &mut Context<Self>) {
        if !self.update_install.installed {
            return;
        }
        if let Some(path) = restart_path(std::env::var_os("APPIMAGE")) {
            cx.set_restart_path(path);
        }
        cx.restart();
    }
}

#[cfg(test)]
mod tests {
    use super::super::book::{CheckShown, VersionView};
    use super::*;
    use junto_client_core::updates::Version;

    fn view(check: CheckShown) -> VersionView {
        VersionView {
            build: Version::new(0, 1, 0),
            check,
            can_check: true,
            checking: false,
        }
    }

    #[test]
    fn the_version_line_states_the_version_and_the_next_step_of_each_way() {
        let newer = || CheckShown::Available {
            version: Version::new(0, 1, 1),
            page: None,
        };
        let line = version_line(&view(CheckShown::Current), &UpdateWay::InApp, false, false, None);
        assert_eq!(line.version, "Junto 0.1.0");
        assert_eq!(
            (line.sentence.as_str(), line.action, line.newer),
            ("Up to date.", None, false)
        );
        let line = version_line(&view(newer()), &UpdateWay::InApp, false, false, None);
        assert_eq!(line.sentence, "Junto 0.1.1 is available.");
        assert_eq!(line.action, Some(VersionAction::Update));
        assert!(line.newer);
        let line = version_line(&view(newer()), &UpdateWay::InApp, true, false, None);
        assert_eq!(line.action, None);
        let line = version_line(&view(newer()), &UpdateWay::InApp, false, true, None);
        assert_eq!(line.sentence, "Junto 0.1.1 is installed. Restart Junto to use it.");
        assert_eq!(line.action, Some(VersionAction::Restart));
        let line = version_line(
            &view(newer()),
            &UpdateWay::InApp,
            false,
            false,
            Some("The update failed: no space."),
        );
        assert_eq!(line.sentence, "The update failed: no space.");
        assert_eq!(line.action, Some(VersionAction::Update));
        // A copy that does not install the update itself names its step and opens the
        // section "Version", which holds the whole steps.
        let line = version_line(&view(newer()), &UpdateWay::SystemPackage, false, false, None);
        assert!(line.sentence.contains("junto_0.1.1_amd64.deb"), "{}", line.sentence);
        assert_eq!(line.action, Some(VersionAction::Details));
        for way in [UpdateWay::MoveFirst, UpdateWay::NotAPackage] {
            let line = version_line(&view(newer()), &way, false, false, None);
            assert_eq!(line.action, Some(VersionAction::Details), "{way:?}");
            assert!(crate::text::book::update_steps(Version::new(0, 1, 1), &way).is_some());
        }
        let line = version_line(
            &view(CheckShown::NotConfigured),
            &UpdateWay::NoSource,
            false,
            false,
            None,
        );
        assert_eq!(line.sentence, "This build does not look for newer versions.");
        assert_eq!(line.action, None);
        assert_eq!(
            crate::text::book::deb_command(Version::new(0, 1, 1)),
            "sudo apt install ./junto_0.1.1_amd64.deb"
        );
    }

    #[test]
    fn an_appimage_restarts_from_its_file_and_another_package_from_its_program() {
        assert_eq!(
            restart_path(Some("/opt/apps/Junto.AppImage".into())),
            Some(PathBuf::from("/opt/apps/Junto.AppImage"))
        );
        assert_eq!(restart_path(Some("".into())), None);
        assert_eq!(restart_path(None), None);
    }
}
