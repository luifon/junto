//! The explicit update action and the status of its background work.
use super::AppState;
use crate::text::book as txt;
use gpui::Context;
use std::{future::Future, pin::Pin, sync::Arc};

/// The work of one update: on success the sentence that states the result.
type Work = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
type Install = Arc<dyn Fn() -> Work + Send + Sync>;
#[derive(Default)]
pub(super) struct InstallState {
    action: Option<Install>,
    working: bool,
    installed: bool,
    status: Option<String>,
}
impl InstallState {
    pub fn new() -> Self {
        let mut state = Self::default();
        if !matches!(
            junto_client_core::releases::HttpSource::compiled(),
            Ok(Some(_))
        ) {
            return state;
        }
        match junto_client_core::releases::PlatformInstaller::running() {
            Ok(installer) if installer.location().is_ok() => {
                state.action = Some(Arc::new(|| {
                    Box::pin(async {
                        junto_client_core::releases::update_running()
                            .await
                            .map(|installed| txt::installed(&installed.previous))
                            .map_err(|error| error.to_string())
                    })
                }));
            }
            Ok(_) => state.status = Some(txt::MOVE_TO_UPDATE.into()),
            Err(_) => state.status = Some(txt::NOT_A_PACKAGE.into()),
        }
        state
    }
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
    #[cfg(feature = "test-support")]
    pub fn set_update_installer(&mut self, action: impl Fn() -> Work + Send + Sync + 'static) {
        self.update_install = InstallState {
            action: Some(Arc::new(action)),
            ..InstallState::default()
        };
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
}
