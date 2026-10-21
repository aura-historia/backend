use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Eq, PartialEq)]
pub(super) struct AwsEmulatorTopology {
    pub(super) services: BTreeSet<String>,
    pub(super) env: BTreeMap<String, String>,
}

impl AwsEmulatorTopology {
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
            "AWS topology requests services not declared by the process fixture: requested={:?}, active={:?}, missing={missing_services:?}",
            self.services,
            active.services,
        );
        for (key, value) in &self.env {
            match active.env.get(key) {
                Some(active_value) if active_value == value => {}
                Some(active_value) => panic!(
                    "Floci topology has conflicting environment value for {key}: requested={value:?}, active={active_value:?}"
                ),
                None => panic!(
                    "Floci topology requests environment variable {key} after the process fixture started; active environment is {:?}",
                    active.env
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AwsEmulatorTopology;

    #[test]
    fn normalizes_service_and_environment_topology() {
        let topology = AwsEmulatorTopology::new(
            &[" S3 ", "opensearch", "s3"],
            &[(" FEATURE_FLAG ", " enabled ")],
        );

        assert_eq!(
            topology.services.into_iter().collect::<Vec<_>>(),
            ["opensearch", "s3"]
        );
        assert_eq!(
            topology.env.get("FEATURE_FLAG"),
            Some(&"enabled".to_owned())
        );
    }

    #[test]
    fn permits_a_subset_of_the_active_topology() {
        let active =
            AwsEmulatorTopology::new(&["s3", "opensearch"], &[("FEATURE_FLAG", "enabled")]);
        let requested = AwsEmulatorTopology::new(&["s3"], &[("FEATURE_FLAG", "enabled")]);

        requested.assert_compatible_with(&active);
    }

    #[test]
    fn rejects_missing_services_and_conflicting_environment() {
        let active = AwsEmulatorTopology::new(&["s3"], &[("FEATURE_FLAG", "enabled")]);

        assert!(
            std::panic::catch_unwind(|| {
                AwsEmulatorTopology::new(&["opensearch"], &[]).assert_compatible_with(&active);
            })
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| {
                AwsEmulatorTopology::new(&["s3"], &[("FEATURE_FLAG", "disabled")])
                    .assert_compatible_with(&active);
            })
            .is_err()
        );
    }
}
