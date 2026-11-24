//! How a control is offered (ADR 0790). An action that the person's permissions do not
//! allow is absent, in bars, headers, panels and menus. An action that the person may take
//! but that the present state does not allow is shown disabled, with its reason in a menu.
//! A surface that shows the same control to every person, such as the record control of a
//! call, states that exception with `Offer::same_for_everyone`.

use gpui::SharedString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Not drawn.
    Absent,
    /// Drawn without its act. The reason is a state of the place, never a permission.
    Unavailable(Option<SharedString>),
    Available,
}

impl Offer {
    /// An action that needs a permission: absent without it.
    pub fn permitted(holds: bool) -> Self {
        if holds { Self::Available } else { Self::Absent }
    }

    /// An action that every person has, unavailable while the state refuses it.
    pub fn now(refusal: Option<impl Into<SharedString>>) -> Self {
        match refusal {
            Some(reason) => Self::Unavailable(Some(reason.into())),
            None => Self::Available,
        }
    }

    /// A permitted action that the state also refuses at times.
    pub fn permitted_now(holds: bool, refusal: Option<impl Into<SharedString>>) -> Self {
        if holds {
            Self::now(refusal)
        } else {
            Self::Absent
        }
    }

    /// The exception: a control that shows the same state to every person, disabled for a
    /// person without the permission, without a sentence about the permission.
    pub fn same_for_everyone(holds: bool) -> Self {
        if holds {
            Self::Available
        } else {
            Self::Unavailable(None)
        }
    }

    pub fn shown(&self) -> bool {
        !matches!(self, Self::Absent)
    }

    pub fn disabled(&self) -> bool {
        !matches!(self, Self::Available)
    }

    pub fn reason(&self) -> Option<SharedString> {
        match self {
            Self::Unavailable(reason) => reason.clone(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Offer;

    #[test]
    fn a_missing_permission_hides_and_a_state_disables_with_its_reason() {
        let absent = Offer::permitted_now(false, Some("No hand is raised."));
        assert!(!absent.shown());
        let waiting = Offer::permitted_now(true, Some("No hand is raised."));
        assert!(waiting.shown() && waiting.disabled());
        assert_eq!(waiting.reason().as_deref(), Some("No hand is raised."));
        let free = Offer::permitted_now(true, None::<&str>);
        assert!(free.shown() && !free.disabled());
        let shared = Offer::same_for_everyone(false);
        assert!(shared.shown() && shared.disabled() && shared.reason().is_none());
    }
}
