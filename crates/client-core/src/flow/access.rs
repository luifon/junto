//! The access flow of one saved server: joining, asking for access, linking this device,
//! logging in, and what a ban or a removal leaves (ADR 0712).
//!
//! This module holds the states, the actions and the rules that decide the next state from
//! an answer of the server. The rules are functions without side effects; the manager runs
//! the requests and applies them.

use junto_protocol::account::RecoveryNotice;
use junto_protocol::ids::{AccountId, DeviceId, JoinRequestId};
use junto_protocol::invite::{JoinRequestState, JoinRequestStatus};
use junto_protocol::keys::PublicKey;
use junto_protocol::secret::{InviteCode, LinkCode, OwnerCode};
use time::OffsetDateTime;

use super::FlowError;
use crate::api::Session;
use crate::book::Membership;
use crate::profile;

/// Why a device that had or asked for access has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Notice {
    /// The account or this device was removed from the server.
    Removed,
    RequestDenied,
    RequestExpired,
    LinkExpired,
    /// The book names an account and the key of this device is not stored any more.
    KeyMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessState {
    /// The key of this device belongs to no account on the server.
    NoAccount { notice: Option<Notice> },
    /// A join request waits for a person with the permission to approve it. The
    /// identifier is absent while the client does not know whether the server received
    /// the request; `Check` finds that out. The name is empty when the client found a
    /// request of this device that its book did not know.
    WaitingForApproval {
        join_request_id: Option<JoinRequestId>,
        name: String,
    },
    /// A device link waits for the approval of a logged-in device. The interface shows
    /// the code and the public key, and the approving device shows the same key.
    WaitingForLink {
        code: LinkCode,
        public_key: PublicKey,
        expires_at: OffsetDateTime,
    },
    /// The device belongs to an account and has no session.
    LoggedOut { account_id: AccountId },
    LoggedIn {
        account_id: AccountId,
        device_id: DeviceId,
        session_expires_at: OffsetDateTime,
        /// The recoveries of the account since the login before (ADR 0112).
        recoveries: Vec<RecoveryNotice>,
        /// Present when the key belonged to another account at the last contact.
        previous_account: Option<AccountId>,
    },
    /// The account is banned. An unban restores it; `Check` finds that out.
    Banned { account_id: Option<AccountId> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    Check,
    ClaimOwner,
    Join,
    RequestAccess,
    RequestLink,
    LogIn,
    LogOut,
    ForgetAccount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessAction {
    /// Asks the server what this device is now: the result of a join request, of a device
    /// link, of a ban, or a login.
    Check,
    /// Presents the owner code of the server (ADR 0110, ADR 0114).
    ClaimOwner {
        code: OwnerCode,
        name: String,
        device_name: String,
    },
    /// Creates an account with an invite code.
    Join {
        code: InviteCode,
        name: String,
        device_name: String,
    },
    /// Asks for access without a code. The request carries the name only.
    RequestAccess {
        name: String,
    },
    /// Asks for a link code that a logged-in device of the account approves.
    RequestLink {
        device_name: String,
    },
    LogIn,
    LogOut,
    /// Deletes the key of this device for the server. The device can then join as a new
    /// account. A banned account stays banned on the server.
    ForgetAccount,
}

impl AccessAction {
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::Check => ActionKind::Check,
            Self::ClaimOwner { .. } => ActionKind::ClaimOwner,
            Self::Join { .. } => ActionKind::Join,
            Self::RequestAccess { .. } => ActionKind::RequestAccess,
            Self::RequestLink { .. } => ActionKind::RequestLink,
            Self::LogIn => ActionKind::LogIn,
            Self::LogOut => ActionKind::LogOut,
            Self::ForgetAccount => ActionKind::ForgetAccount,
        }
    }

    /// The action with its names checked by the rule of the server, before any request.
    pub(crate) fn checked(self) -> Result<Self, FlowError> {
        Ok(match self {
            Self::ClaimOwner {
                code,
                name,
                device_name,
            } => Self::ClaimOwner {
                code,
                name: profile::account_name(&name)?,
                device_name: profile::device_name(&device_name)?,
            },
            Self::Join {
                code,
                name,
                device_name,
            } => Self::Join {
                code,
                name: profile::account_name(&name)?,
                device_name: profile::device_name(&device_name)?,
            },
            Self::RequestAccess { name } => Self::RequestAccess {
                name: profile::account_name(&name)?,
            },
            Self::RequestLink { device_name } => Self::RequestLink {
                device_name: profile::device_name(&device_name)?,
            },
            other => other,
        })
    }
}

/// The actions that a state accepts.
pub fn actions(state: &AccessState) -> &'static [ActionKind] {
    use ActionKind as K;
    match state {
        AccessState::NoAccount { .. } => &[
            K::Check,
            K::Join,
            K::RequestAccess,
            K::RequestLink,
            K::ClaimOwner,
        ],
        AccessState::WaitingForApproval { .. } => {
            &[K::Check, K::Join, K::ClaimOwner, K::ForgetAccount]
        }
        AccessState::WaitingForLink { .. } => &[
            K::Check,
            K::RequestLink,
            K::Join,
            K::RequestAccess,
            K::ClaimOwner,
            K::ForgetAccount,
        ],
        AccessState::LoggedOut { .. } => &[K::LogIn],
        AccessState::LoggedIn { .. } => &[K::Check, K::LogOut],
        AccessState::Banned { .. } => &[K::Check, K::ForgetAccount],
    }
}

/// The state of a server before the client has contacted it, from the stored membership.
pub fn initial(membership: &Membership) -> AccessState {
    match membership {
        Membership::None => AccessState::NoAccount { notice: None },
        Membership::Removed => AccessState::NoAccount {
            notice: Some(Notice::Removed),
        },
        Membership::JoinRequested {
            join_request_id,
            name,
        } => AccessState::WaitingForApproval {
            join_request_id: *join_request_id,
            name: name.clone(),
        },
        Membership::Member { account_id, .. } => AccessState::LoggedOut {
            account_id: *account_id,
        },
        Membership::Banned { account_id } => AccessState::Banned {
            account_id: *account_id,
        },
    }
}

/// The answer of the server to a login attempt of this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    LoggedIn(Session),
    /// `unknown_device`: the key belongs to no account.
    UnknownDevice,
    /// `account_banned`.
    Banned,
}

/// A state and the membership that the book stores with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settled {
    pub(crate) state: AccessState,
    pub(crate) membership: Membership,
}

fn account_of(membership: &Membership) -> Option<AccountId> {
    match membership {
        Membership::Member { account_id, .. } => Some(*account_id),
        Membership::Banned { account_id } => *account_id,
        Membership::None | Membership::Removed | Membership::JoinRequested { .. } => None,
    }
}

/// The state after a login attempt.
pub(crate) fn after_probe(
    before: &AccessState,
    membership: &Membership,
    probe: Probe,
    now: OffsetDateTime,
) -> Settled {
    match probe {
        Probe::LoggedIn(session) => {
            let previous_account =
                account_of(membership).filter(|account_id| *account_id != session.account_id);
            Settled {
                state: AccessState::LoggedIn {
                    account_id: session.account_id,
                    device_id: session.device_id,
                    session_expires_at: session.expires_at,
                    recoveries: session.recoveries,
                    previous_account,
                },
                membership: Membership::Member {
                    account_id: session.account_id,
                    device_id: session.device_id,
                },
            }
        }
        Probe::Banned => {
            let account_id = account_of(membership);
            Settled {
                state: AccessState::Banned { account_id },
                membership: Membership::Banned { account_id },
            }
        }
        Probe::UnknownDevice => match membership {
            Membership::Member { .. } | Membership::Banned { .. } | Membership::Removed => {
                Settled {
                    state: AccessState::NoAccount {
                        notice: Some(Notice::Removed),
                    },
                    membership: Membership::Removed,
                }
            }
            Membership::JoinRequested { .. } => Settled {
                state: AccessState::NoAccount { notice: None },
                membership: Membership::None,
            },
            Membership::None => match before {
                AccessState::WaitingForLink { expires_at, .. } if *expires_at > now => Settled {
                    state: before.clone(),
                    membership: Membership::None,
                },
                AccessState::WaitingForLink { .. } => Settled {
                    state: AccessState::NoAccount {
                        notice: Some(Notice::LinkExpired),
                    },
                    membership: Membership::None,
                },
                _ => Settled {
                    state: AccessState::NoAccount { notice: None },
                    membership: Membership::None,
                },
            },
        },
    }
}

/// The state after the server told the result of the join request of this device. `None`
/// when the request was approved: the device then logs in.
pub(crate) fn after_request_status(status: &JoinRequestStatus, name: &str) -> Option<Settled> {
    let ended = |notice| Settled {
        state: AccessState::NoAccount {
            notice: Some(notice),
        },
        membership: Membership::None,
    };
    match status.state {
        JoinRequestState::Pending => Some(Settled {
            state: AccessState::WaitingForApproval {
                join_request_id: Some(status.id),
                name: name.to_owned(),
            },
            membership: Membership::JoinRequested {
                join_request_id: Some(status.id),
                name: name.to_owned(),
            },
        }),
        JoinRequestState::Approved => None,
        JoinRequestState::Denied => Some(ended(Notice::RequestDenied)),
        JoinRequestState::Expired => Some(ended(Notice::RequestExpired)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::SecretToken;
    use junto_protocol::ids::RecoveryId;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn session(account_id: AccountId) -> Session {
        Session {
            token: SecretToken::new("jst_test".to_owned()),
            expires_at: now() + time::Duration::days(30),
            account_id,
            device_id: DeviceId::generate(),
            recoveries: Vec::new(),
        }
    }

    fn states() -> Vec<AccessState> {
        vec![
            AccessState::NoAccount { notice: None },
            AccessState::WaitingForApproval {
                join_request_id: Some(JoinRequestId::generate()),
                name: "Ada".to_owned(),
            },
            AccessState::WaitingForLink {
                code: LinkCode::new("ABCD-EFGH"),
                public_key: PublicKey::from_bytes([1; 32]),
                expires_at: now() + time::Duration::minutes(10),
            },
            AccessState::LoggedOut {
                account_id: AccountId::generate(),
            },
            AccessState::LoggedIn {
                account_id: AccountId::generate(),
                device_id: DeviceId::generate(),
                session_expires_at: now(),
                recoveries: Vec::new(),
                previous_account: None,
            },
            AccessState::Banned { account_id: None },
        ]
    }

    #[test]
    fn every_state_accepts_an_action_and_no_action_twice() {
        for state in states() {
            let accepted = actions(&state);
            assert!(!accepted.is_empty(), "{state:?}");
            let mut unique = accepted.to_vec();
            unique.sort_by_key(|kind| *kind as u8);
            unique.dedup();
            assert_eq!(unique.len(), accepted.len(), "{state:?}");
        }
    }

    #[test]
    fn the_actions_of_each_state() {
        use ActionKind as K;
        let all = states();
        let accepts = |index: usize, kind: K| actions(&all[index]).contains(&kind);
        for kind in [
            K::Join,
            K::RequestAccess,
            K::RequestLink,
            K::ClaimOwner,
            K::Check,
        ] {
            assert!(accepts(0, kind), "{kind:?}");
        }
        assert!(!accepts(0, K::LogIn) && !accepts(0, K::LogOut));
        assert!(accepts(1, K::Check) && accepts(1, K::Join) && accepts(1, K::ForgetAccount));
        assert!(!accepts(1, K::RequestAccess));
        assert!(accepts(2, K::Check) && accepts(2, K::RequestLink));
        assert_eq!(actions(&all[3]), [K::LogIn]);
        assert_eq!(actions(&all[4]), [K::Check, K::LogOut]);
        assert_eq!(actions(&all[5]), [K::Check, K::ForgetAccount]);
        for state in &all {
            let joined = matches!(
                state,
                AccessState::LoggedIn { .. } | AccessState::LoggedOut { .. }
            );
            for kind in [K::Join, K::RequestAccess, K::RequestLink, K::ClaimOwner] {
                assert!(!(joined && actions(state).contains(&kind)), "{state:?}");
            }
            assert_eq!(
                actions(state).contains(&K::ForgetAccount),
                matches!(
                    state,
                    AccessState::Banned { .. }
                        | AccessState::WaitingForApproval { .. }
                        | AccessState::WaitingForLink { .. }
                ),
                "{state:?}"
            );
        }
    }

    #[test]
    fn the_stored_membership_gives_the_state_before_any_contact() {
        let account_id = AccountId::generate();
        let request = Some(JoinRequestId::generate());
        let cases = [
            (Membership::None, AccessState::NoAccount { notice: None }),
            (
                Membership::Removed,
                AccessState::NoAccount {
                    notice: Some(Notice::Removed),
                },
            ),
            (
                Membership::JoinRequested {
                    join_request_id: request,
                    name: "Ada".to_owned(),
                },
                AccessState::WaitingForApproval {
                    join_request_id: request,
                    name: "Ada".to_owned(),
                },
            ),
            (
                Membership::Member {
                    account_id,
                    device_id: DeviceId::generate(),
                },
                AccessState::LoggedOut { account_id },
            ),
            (
                Membership::Banned {
                    account_id: Some(account_id),
                },
                AccessState::Banned {
                    account_id: Some(account_id),
                },
            ),
        ];
        for (membership, expected) in cases {
            assert_eq!(initial(&membership), expected);
        }
    }

    #[test]
    fn a_login_makes_the_device_a_member_from_every_state() {
        let account_id = AccountId::generate();
        let created = session(account_id);
        for before in states() {
            for membership in [Membership::None, Membership::Removed] {
                let settled = after_probe(
                    &before,
                    &membership,
                    Probe::LoggedIn(created.clone()),
                    now(),
                );
                assert_eq!(
                    settled.membership,
                    Membership::Member {
                        account_id,
                        device_id: created.device_id
                    }
                );
                assert!(matches!(
                    settled.state,
                    AccessState::LoggedIn {
                        previous_account: None,
                        ..
                    }
                ));
            }
        }
    }

    #[test]
    fn a_login_shows_recoveries_and_a_change_of_the_account() {
        let (stored, other) = (AccountId::generate(), AccountId::generate());
        let membership = Membership::Member {
            account_id: stored,
            device_id: DeviceId::generate(),
        };
        let mut created = session(other);
        created.recoveries = vec![RecoveryNotice {
            id: RecoveryId::generate(),
            device_id: None,
            performed_by: None,
            performed_at: now(),
        }];
        let before = AccessState::LoggedOut { account_id: stored };
        let settled = after_probe(
            &before,
            &membership,
            Probe::LoggedIn(created.clone()),
            now(),
        );
        let AccessState::LoggedIn {
            account_id,
            recoveries,
            previous_account,
            ..
        } = settled.state
        else {
            panic!("not logged in");
        };
        assert_eq!(account_id, other);
        assert_eq!(recoveries, created.recoveries);
        assert_eq!(previous_account, Some(stored));

        let same = after_probe(
            &before,
            &membership,
            Probe::LoggedIn(session(stored)),
            now(),
        );
        assert!(matches!(
            same.state,
            AccessState::LoggedIn {
                previous_account: None,
                ..
            }
        ));
    }

    #[test]
    fn a_ban_keeps_the_account_and_an_unban_logs_in_again() {
        let account_id = AccountId::generate();
        let member = Membership::Member {
            account_id,
            device_id: DeviceId::generate(),
        };
        let before = AccessState::LoggedOut { account_id };
        let banned = after_probe(&before, &member, Probe::Banned, now());
        assert_eq!(
            banned.state,
            AccessState::Banned {
                account_id: Some(account_id)
            }
        );
        assert_eq!(
            banned.membership,
            Membership::Banned {
                account_id: Some(account_id)
            }
        );

        let again = after_probe(&banned.state, &banned.membership, Probe::Banned, now());
        assert_eq!(again, banned);

        let unbanned = after_probe(
            &banned.state,
            &banned.membership,
            Probe::LoggedIn(session(account_id)),
            now(),
        );
        assert!(matches!(
            unbanned.state,
            AccessState::LoggedIn {
                previous_account: None,
                ..
            }
        ));

        let unknown_account = after_probe(
            &AccessState::NoAccount { notice: None },
            &Membership::None,
            Probe::Banned,
            now(),
        );
        assert_eq!(
            unknown_account.state,
            AccessState::Banned { account_id: None }
        );
    }

    #[test]
    fn an_unknown_key_of_a_member_or_a_banned_account_means_removed() {
        let account_id = AccountId::generate();
        let cases = [
            Membership::Member {
                account_id,
                device_id: DeviceId::generate(),
            },
            Membership::Banned {
                account_id: Some(account_id),
            },
            Membership::Removed,
        ];
        for membership in cases {
            let settled = after_probe(
                &initial(&membership),
                &membership,
                Probe::UnknownDevice,
                now(),
            );
            assert_eq!(
                settled.state,
                AccessState::NoAccount {
                    notice: Some(Notice::Removed)
                }
            );
            assert_eq!(settled.membership, Membership::Removed);
        }
    }

    #[test]
    fn an_unknown_key_without_an_account_changes_nothing_but_an_expired_link() {
        let plain = after_probe(
            &AccessState::NoAccount { notice: None },
            &Membership::None,
            Probe::UnknownDevice,
            now(),
        );
        assert_eq!(plain.state, AccessState::NoAccount { notice: None });
        assert_eq!(plain.membership, Membership::None);

        let waiting = AccessState::WaitingForLink {
            code: LinkCode::new("ABCD-EFGH"),
            public_key: PublicKey::from_bytes([1; 32]),
            expires_at: now() + time::Duration::seconds(1),
        };
        let still = after_probe(&waiting, &Membership::None, Probe::UnknownDevice, now());
        assert_eq!(still.state, waiting);

        let later = now() + time::Duration::seconds(1);
        let expired = after_probe(&waiting, &Membership::None, Probe::UnknownDevice, later);
        assert_eq!(
            expired.state,
            AccessState::NoAccount {
                notice: Some(Notice::LinkExpired)
            }
        );

        let request = Membership::JoinRequested {
            join_request_id: Some(JoinRequestId::generate()),
            name: "Ada".to_owned(),
        };
        let gone = after_probe(&initial(&request), &request, Probe::UnknownDevice, now());
        assert_eq!(gone.state, AccessState::NoAccount { notice: None });
        assert_eq!(gone.membership, Membership::None);
    }

    #[test]
    fn the_result_of_a_join_request_decides_the_next_state() {
        let id = JoinRequestId::generate();
        let status = |state| JoinRequestStatus {
            id,
            state,
            account_id: None,
        };
        let pending = after_request_status(&status(JoinRequestState::Pending), "Ada").unwrap();
        assert_eq!(
            pending.state,
            AccessState::WaitingForApproval {
                join_request_id: Some(id),
                name: "Ada".to_owned()
            }
        );
        assert_eq!(
            pending.membership,
            Membership::JoinRequested {
                join_request_id: Some(id),
                name: "Ada".to_owned()
            }
        );
        assert!(after_request_status(&status(JoinRequestState::Approved), "Ada").is_none());
        for (state, notice) in [
            (JoinRequestState::Denied, Notice::RequestDenied),
            (JoinRequestState::Expired, Notice::RequestExpired),
        ] {
            let settled = after_request_status(&status(state), "Ada").unwrap();
            assert_eq!(
                settled.state,
                AccessState::NoAccount {
                    notice: Some(notice)
                }
            );
            assert_eq!(settled.membership, Membership::None);
        }
    }

    #[test]
    fn names_are_checked_before_any_request() {
        let join = AccessAction::Join {
            code: InviteCode::new("ABCD-EFGH-JKMN-PQRS"),
            name: "  Ada ".to_owned(),
            device_name: " Laptop ".to_owned(),
        };
        assert_eq!(
            join.checked().unwrap(),
            AccessAction::Join {
                code: InviteCode::new("ABCD-EFGH-JKMN-PQRS"),
                name: "Ada".to_owned(),
                device_name: "Laptop".to_owned(),
            }
        );
        let refused = [
            AccessAction::RequestAccess {
                name: "  ".to_owned(),
            },
            AccessAction::RequestLink {
                device_name: "a\nb".to_owned(),
            },
            AccessAction::ClaimOwner {
                code: OwnerCode::new("ABCD"),
                name: "x".repeat(65),
                device_name: String::new(),
            },
        ];
        for action in refused {
            assert!(matches!(action.checked(), Err(FlowError::InvalidInput(_))));
        }
        assert_eq!(AccessAction::Check.checked().unwrap(), AccessAction::Check);
    }

    #[test]
    fn formatting_an_action_or_a_state_shows_no_code() {
        let join = AccessAction::Join {
            code: InviteCode::new("SECRET-CODE"),
            name: "Ada".to_owned(),
            device_name: String::new(),
        };
        assert!(!format!("{join:?}").contains("SECRET"));
        let waiting = AccessState::WaitingForLink {
            code: LinkCode::new("SECRET-CODE"),
            public_key: PublicKey::from_bytes([1; 32]),
            expires_at: now(),
        };
        assert!(!format!("{waiting:?}").contains("SECRET"));
    }
}
