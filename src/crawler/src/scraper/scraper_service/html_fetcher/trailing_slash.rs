//! Hosts that redirect a path to its trailing-slash form (WordPress and
//! WooCommerce by default). The crawler stores a URL with the trailing slash
//! the spider fetched it with, so such a host costs a redirect round trip only
//! for URLs stored without the slash: where the shop links that spelling, and
//! rows stored before the crawler kept the slash, until the spider rediscovers
//! them. Remembering the host lets later fetches request the slashed form
//! directly; stored URLs keep their spelling.

use dashmap::DashSet;
use tracing::debug;
use url::Url;

/// Bounds the memory one long-running process spends on hosts.
const MAX_HOSTS: usize = 10_000;

#[derive(Default)]
pub(super) struct TrailingSlashHosts {
    hosts: DashSet<String>,
}

impl TrailingSlashHosts {
    /// The form to request first: `url` with a trailing slash when its host is
    /// known to redirect there, `None` when `url` should be requested as is.
    pub(super) fn preferred_form(&self, url: &Url) -> Option<Url> {
        if !self.hosts.contains(url.host_str()?) {
            return None;
        }
        with_trailing_slash(url)
    }

    /// Learns from one redirect hop: a hop that only appends the trailing
    /// slash marks the host, a hop that only removes it unmarks the host.
    pub(super) fn observe_redirect(&self, from: &Url, to: &Url) {
        let Some(host) = from.host_str() else {
            return;
        };
        if with_trailing_slash(from).as_ref() == Some(to) {
            if self.hosts.len() < MAX_HOSTS && self.hosts.insert(host.to_owned()) {
                debug!(
                    host,
                    "Host redirects to trailing-slash paths; requesting that form directly"
                );
            }
        } else if with_trailing_slash(to).as_ref() == Some(from)
            && self.hosts.remove(host).is_some()
        {
            debug!(
                host,
                "Host redirects trailing-slash paths away; requesting stored URLs as they are"
            );
        }
    }
}

/// `url` with `/` appended to its path, or `None` when the path already ends
/// with one or names a file (a last segment with an extension).
fn with_trailing_slash(url: &Url) -> Option<Url> {
    let path = url.path();
    if path.ends_with('/') {
        return None;
    }
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    if last_segment.contains('.') {
        return None;
    }
    let mut slashed = url.clone();
    slashed.set_path(&format!("{path}/"));
    Some(slashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    fn hosts_after_redirect(from: &str, to: &str) -> TrailingSlashHosts {
        let hosts = TrailingSlashHosts::default();
        hosts.observe_redirect(&url(from), &url(to));
        hosts
    }

    #[test]
    fn should_prefer_the_slashed_form_after_the_host_redirected_to_it() {
        let hosts = hosts_after_redirect(
            "https://shop.test/product/a",
            "https://shop.test/product/a/",
        );

        assert_eq!(
            hosts.preferred_form(&url("https://shop.test/product/b?x=1")),
            Some(url("https://shop.test/product/b/?x=1"))
        );
    }

    #[test]
    fn should_not_learn_from_a_redirect_that_changes_more_than_the_slash() {
        for to in [
            "https://shop.test/product/b/",
            "https://shop.test/product/a/?x=1",
            "https://www.shop.test/product/a/",
            "http://shop.test/product/a/",
        ] {
            let hosts = hosts_after_redirect("https://shop.test/product/a", to);

            assert_eq!(
                hosts.preferred_form(&url("https://shop.test/product/c")),
                None,
                "{to}"
            );
        }
    }

    #[test]
    fn should_keep_other_hosts_files_and_slashed_paths_as_they_are() {
        let hosts = hosts_after_redirect(
            "https://shop.test/product/a",
            "https://shop.test/product/a/",
        );

        for raw in [
            "https://other.test/product/b",
            "https://shop.test/stock/view/16891.asp",
            "https://shop.test/product/b/",
        ] {
            assert_eq!(hosts.preferred_form(&url(raw)), None, "{raw}");
        }
    }

    #[test]
    fn should_forget_the_host_when_it_redirects_a_slashed_path_away() {
        let hosts = hosts_after_redirect(
            "https://shop.test/product/a",
            "https://shop.test/product/a/",
        );

        hosts.observe_redirect(
            &url("https://shop.test/product/b/"),
            &url("https://shop.test/product/b"),
        );

        assert_eq!(
            hosts.preferred_form(&url("https://shop.test/product/c")),
            None
        );
    }
}
