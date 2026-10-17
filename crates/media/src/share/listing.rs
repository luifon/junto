//! The rules that decide which applications a share offers, on the systems where the client
//! lists them itself: macOS (ADR 0736) and Windows (ADR 0706).

use super::{ApplicationId, ApplicationSource};

/// What the client is to the system: its process, the identity of its program, and its name.
/// macOS gives a bundle identifier and a process name; Windows gives the path of the
/// program file and its file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Own {
    pub process_id: i32,
    pub bundle_id: String,
    pub name: String,
}

impl Own {
    /// Whether an application is the client: this process, or another start of the same
    /// program. The exclusion of the own sound has no effect on a capture that selects an
    /// application of the same program, so none of them is offered (ADR 0502).
    pub(crate) fn is(&self, process_id: i32, bundle_id: &str, name: &str) -> bool {
        process_id == self.process_id || (bundle_id == self.bundle_id && name == self.name)
    }
}

/// The applications whose sound can be shared alone: every listed application with a name
/// except the client, once each.
pub(crate) fn shareable_applications(
    listed: Vec<(ApplicationId, String)>,
    own: &Own,
) -> Vec<ApplicationSource> {
    let mut applications: Vec<ApplicationSource> = Vec::new();
    for (id, name) in listed {
        if name.is_empty()
            || own.is(id.process_id, &id.bundle_id, &name)
            || applications.iter().any(|held| held.id == id)
        {
            continue;
        }
        applications.push(ApplicationSource { id, name });
    }
    applications.sort_by(|one, other| {
        one.name
            .to_lowercase()
            .cmp(&other.name.to_lowercase())
            .then(one.id.process_id.cmp(&other.id.process_id))
    });
    applications
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        Own {
            process_id: 100,
            bundle_id: "chat.junto.client".to_owned(),
            name: "Junto".to_owned(),
        }
    }

    fn application(process_id: i32, bundle_id: &str) -> ApplicationId {
        ApplicationId {
            process_id,
            bundle_id: bundle_id.to_owned(),
        }
    }

    #[test]
    fn the_client_is_its_own_process_and_every_other_start_of_the_same_program() {
        let own = own();
        assert!(own.is(100, "chat.junto.client", "Junto"));
        // A second instance of the application.
        assert!(own.is(200, "chat.junto.client", "Junto"));
        // Another application, also one that took the process identifier meanwhile.
        assert!(!own.is(200, "example.player", "Player"));
        assert!(own.is(100, "example.player", "Player"));
        // A build without a bundle has no bundle identifier: the name decides.
        let unbundled = Own {
            process_id: 100,
            bundle_id: String::new(),
            name: "junto".to_owned(),
        };
        assert!(unbundled.is(300, "", "junto"));
        assert!(!unbundled.is(300, "", "ffplay"));
        assert!(!unbundled.is(300, "example.player", "junto"));
    }

    #[test]
    fn the_client_is_not_among_the_applications_whose_sound_is_offered() {
        let listed = vec![
            (application(9, "example.zeta"), "zeta".to_owned()),
            (application(100, "chat.junto.client"), "Junto".to_owned()),
            (application(7, "example.player"), "Player".to_owned()),
            (application(200, "chat.junto.client"), "Junto".to_owned()),
            (application(7, "example.player"), "Player".to_owned()),
            (application(8, "example.nameless"), String::new()),
        ];
        let names: Vec<String> = shareable_applications(listed, &own())
            .into_iter()
            .map(|application| application.name)
            .collect();
        assert_eq!(names, ["Player", "zeta"]);
    }
}
