//! The explicit update action and the status of its background work.
use super::AppState;
use crate::text::book as txt;
use gpui::Context;
use std::{future::Future, pin::Pin, sync::Arc};

type Work = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
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
        #[cfg(target_os = "macos")]
        let mut state = Self::default();
        #[cfg(not(target_os = "macos"))]
        let state = Self::default();
        #[cfg(target_os = "macos")]
        if matches!(
            junto_client_core::releases::HttpSource::compiled(),
            Ok(Some(_))
        ) && let Ok(installer) = junto_client_core::releases::macos::MacInstaller::running()
        {
            if installer.location().is_ok() {
                state.action = Some(Arc::new(|| {
                    Box::pin(async {
                        junto_client_core::releases::macos::update_running()
                            .await
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                }));
            } else {
                state.status = Some(txt::MOVE_TO_UPDATE.into());
            }
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
                this.update_install.installed = matches!(result, Ok(Ok(())));
                this.update_install.status = Some(match result {
                    Ok(Ok(())) => txt::INSTALLED.into(),
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
