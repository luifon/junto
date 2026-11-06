use super::AppState;
use gpui::Context;
use junto_client_core::startup::{KeptStartup, Startup, Status};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct StartupView {
    pub status: Option<Status>,
    /// The name of the profile of the entry, when it is not the normal profile.
    pub profile: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
}

pub(super) struct StartupState {
    service: Arc<dyn Startup>,
    number: u64,
    wanted: Option<bool>,
    view: StartupView,
}

impl Default for StartupState {
    fn default() -> Self {
        Self {
            service: Arc::new(KeptStartup::default()),
            number: 0,
            wanted: None,
            view: StartupView {
                status: Some(Status::Off),
                profile: None,
                busy: false,
                error: None,
            },
        }
    }
}

impl AppState {
    pub fn startup(&self) -> &StartupView {
        &self.startup.view
    }

    pub fn use_startup_service(&mut self, service: Arc<dyn Startup>, cx: &mut Context<Self>) {
        self.startup.service = service;
        self.startup.view.busy = false;
        self.startup.view.status = None;
        self.startup.view.error = None;
        self.startup.wanted = None;
        self.refresh_startup(cx);
    }

    pub fn refresh_startup(&mut self, cx: &mut Context<Self>) {
        self.change_startup(None, cx);
    }

    pub fn set_start_at_login(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self
            .startup
            .view
            .status
            .is_none_or(|status| matches!(status, Status::Unavailable(_)))
        {
            return;
        }
        self.change_startup(Some(enabled), cx);
    }

    pub fn retry_startup(&mut self, cx: &mut Context<Self>) {
        self.change_startup(self.startup.wanted, cx);
    }

    fn change_startup(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        if self.startup.view.busy {
            return;
        }
        self.startup.number = self.startup.number.wrapping_add(1);
        let number = self.startup.number;
        self.startup.view.busy = true;
        if let Some(enabled) = enabled {
            self.startup.wanted = Some(enabled);
        }
        let service = self.startup.service.clone();
        let changing = cx.background_executor().spawn(async move {
            let status = match enabled {
                Some(enabled) => service.set(enabled),
                None => service.status(),
            };
            (status, service.profile_name())
        });
        cx.spawn(async move |this, cx| {
            let (result, profile) = changing.await;
            let _ = this.update(cx, |this, cx| {
                if this.startup.number != number {
                    return;
                }
                this.startup.view.busy = false;
                this.startup.view.profile = profile;
                match result {
                    Ok(status) => {
                        this.startup.view.status = Some(status);
                        if this
                            .startup
                            .wanted
                            .is_none_or(|wanted| status.requested() == wanted)
                        {
                            this.startup.wanted = None;
                            this.startup.view.error = None;
                        } else if enabled.is_some() {
                            this.startup.view.error =
                                Some(crate::text::LOGIN_NOT_CHANGED.to_owned());
                        }
                    }
                    Err(error) => this.startup.view.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn open_login_approval(&mut self, cx: &mut Context<Self>) {
        if self.startup.view.status != Some(Status::RequiresApproval) {
            return;
        }
        let number = self.startup.number;
        let service = self.startup.service.clone();
        let opening = cx
            .background_executor()
            .spawn(async move { service.open_approval() });
        cx.spawn(async move |this, cx| {
            if let Err(error) = opening.await {
                let _ = this.update(cx, |this, cx| {
                    if this.startup.number == number {
                        this.startup.view.error = Some(error.to_string());
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
}
