use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Eq, PartialEq)]
pub(super) struct LocalStackTopology {
    pub(super) services: BTreeSet<String>,
    pub(super) env: BTreeMap<String, String>,
}

impl LocalStackTopology {
    pub(super) fn new(services: &[&str], extra_env_vars: &[(&str, &str)]) -> Self {
        let services = services
            .iter()
            .map(|service| service.trim().to_ascii_lowercase())
            .collect();
        let env = extra_env_vars
            .iter()
            .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
            .collect();
        Self { services, env }
    }

    pub(super) fn assert_compatible_with(&self, active: &Self) {
        let missing_services = self
            .services
            .difference(&active.services)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing_services.is_empty(),
            "LocalStack topology requests services not enabled by the process fixture: requested={:?}, active={:?}, missing={missing_services:?}",
            self.services,
            active.services,
        );
        for (key, value) in &self.env {
            match active.env.get(key) {
                Some(active_value) if active_value == value => {}
                Some(active_value) => panic!(
                    "LocalStack topology has conflicting environment value for {key}: requested={value:?}, active={active_value:?}"
                ),
                None => panic!(
                    "LocalStack topology requests environment variable {key} after the process fixture started; active environment is {:?}",
                    active.env
                ),
            }
        }
    }
}
