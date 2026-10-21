//! Compiles robots.txt rules into regular expressions over full URLs that
//! match the disallowed URLs, for crawl engines whose only pre-fetch veto is a
//! regex deny list.
//!
//! The `regex` crate has no lookaround, so an `Allow` exception can only be
//! carved out of a literal `Disallow` path prefix, and only when the `Allow`
//! is a literal path prefix too (it may end in `$`). Other exceptions are not
//! carved out: the deny list then blocks more than robots.txt does, never
//! less.

use super::{RobotsRules, Rule};
use std::collections::BTreeMap;

/// Matches the scheme and authority of an absolute URL, so a rule pattern
/// applies from the start of the path.
const URL_ORIGIN: &str = "^[A-Za-z][A-Za-z0-9+.-]*://[^/?#]*";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenyList {
    /// Regular expressions; a URL matching any of them is disallowed.
    pub patterns: Vec<String>,
    /// `Disallow` patterns blocking URLs robots.txt allows: an `Allow`
    /// written as their exception (its pattern extends the `Disallow`) could
    /// not be carved out. Incidental overlaps, such as `Allow:
    /// /*/products/account` with `Disallow: /admin`, are blocked silently.
    pub failing_closed: Vec<String>,
}

impl RobotsRules {
    pub fn deny_list(&self) -> DenyList {
        let mut deny_list = DenyList::default();
        for disallow in self.rules.iter().filter(|rule| !rule.allow) {
            let (expressible, inexpressible): (Vec<&Rule>, Vec<&Rule>) = self
                .rules
                .iter()
                .filter(|rule| {
                    rule.allow
                        && rule.pattern.len() >= disallow.pattern.len()
                        && may_overlap(&disallow.pattern, &rule.pattern)
                })
                .partition(|allow| literal_suffix(&disallow.pattern, &allow.pattern).is_some());
            let Some(pattern) = carve_exceptions(&disallow.pattern, &expressible) else {
                continue;
            };
            deny_list.patterns.push(pattern);
            let stem = disallow.pattern.trim_end_matches('*');
            if inexpressible
                .iter()
                .any(|allow| allow.pattern.starts_with(stem))
            {
                deny_list.failing_closed.push(disallow.pattern.clone());
            }
        }
        deny_list
    }
}

/// For a literal `disallow`, the part of a literal `allow` (optionally ending
/// in `$`) below it, and whether that part must match exactly.
fn literal_suffix<'a>(disallow: &str, allow: &'a str) -> Option<(&'a str, bool)> {
    if !is_literal(disallow) {
        return None;
    }
    let (literal, exact) = match allow.strip_suffix('$') {
        Some(literal) => (literal, true),
        None => (allow, false),
    };
    if !is_literal(literal) {
        return None;
    }
    literal.strip_prefix(disallow).map(|suffix| (suffix, exact))
}

/// The regex for `disallow` minus its expressible `exceptions`, or `None`
/// when they allow everything it covers.
fn carve_exceptions(disallow: &str, exceptions: &[&Rule]) -> Option<String> {
    if exceptions.is_empty() {
        return Some(format!("{URL_ORIGIN}{}", pattern_regex(disallow)));
    }
    let mut allowed = AllowTrie::default();
    for exception in exceptions {
        if let Some((suffix, exact)) = literal_suffix(disallow, &exception.pattern) {
            allowed.insert(suffix, exact);
        }
    }
    allowed
        .disallowed_suffixes()
        .map(|suffixes| format!("{URL_ORIGIN}{}{suffixes}", literal_regex(disallow)))
}

/// The literal path suffixes `Allow` rules exempt below one `Disallow`.
#[derive(Default)]
struct AllowTrie {
    children: BTreeMap<char, AllowTrie>,
    /// Every path continuing from here is allowed.
    allows_subtree: bool,
    /// A path ending exactly here is allowed.
    allows_exact: bool,
}

impl AllowTrie {
    fn insert(&mut self, suffix: &str, exact: bool) {
        let mut node = self;
        for ch in suffix.chars() {
            node = node.children.entry(ch).or_default();
        }
        if exact {
            node.allows_exact = true;
        } else {
            node.allows_subtree = true;
        }
    }

    /// A regex matching the suffixes this node does not allow, or `None`
    /// when it allows every suffix.
    fn disallowed_suffixes(&self) -> Option<String> {
        if self.allows_subtree {
            return None;
        }
        let mut branches = Vec::new();
        if !self.allows_exact {
            branches.push("$".to_owned());
        }
        if self.children.is_empty() {
            branches.push(".".to_owned());
        } else {
            let listed: String = self
                .children
                .keys()
                .map(|ch| regex::escape(&ch.to_string()))
                .collect();
            branches.push(format!("[^{listed}]"));
        }
        for (ch, child) in &self.children {
            if let Some(rest) = child.disallowed_suffixes() {
                branches.push(format!("{}{rest}", regex::escape(&ch.to_string())));
            }
        }
        Some(format!("(?:{})", branches.join("|")))
    }
}

fn is_literal(pattern: &str) -> bool {
    !pattern.contains(['*', '$'])
}

/// Two rules can only match one path when their literal leads, the parts
/// before the first wildcard, agree.
fn may_overlap(left: &str, right: &str) -> bool {
    let lead = |pattern: &str| {
        pattern
            .split(['*', '$'])
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    let (left, right) = (lead(left), lead(right));
    left.starts_with(&right) || right.starts_with(&left)
}

/// A rule pattern as a regex anchored at the path start: `*` matches any
/// sequence and a trailing `$` anchors the end.
fn pattern_regex(pattern: &str) -> String {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(pattern) => (pattern, true),
        None => (pattern, false),
    };
    let body = pattern
        .split('*')
        .map(literal_regex)
        .collect::<Vec<_>>()
        .join(".*");
    if anchored { format!("{body}$") } else { body }
}

/// A literal pattern part as a regex. Patterns carry upper-case percent
/// escapes, while crawl engines match URLs with the escapes as written, so
/// each escape matches either hex case.
fn literal_regex(literal: &str) -> String {
    let chars: Vec<char> = literal.chars().collect();
    let mut regex = String::with_capacity(literal.len());
    let mut escape_digits = 0_u8;
    for (index, &ch) in chars.iter().enumerate() {
        if escape_digits > 0 && ch.is_ascii_alphabetic() {
            regex.push_str(&format!("[{ch}{}]", ch.to_ascii_lowercase()));
        } else {
            regex.push_str(&regex::escape(&ch.to_string()));
        }
        let starts_escape = ch == '%'
            && chars
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(char::is_ascii_hexdigit));
        escape_digits = if starts_escape {
            2
        } else {
            escape_digits.saturating_sub(1)
        };
    }
    regex
}

#[cfg(test)]
mod tests {
    use super::super::RobotsRules;
    use regex::RegexSet;
    use url::Url;

    /// The deny list must refuse exactly what the rules disallow.
    fn assert_matches_rules(robots: &str, paths: &[&str]) {
        let rules = RobotsRules::parse(robots);
        let deny_list = rules.deny_list();
        let set = RegexSet::new(&deny_list.patterns).unwrap();
        for path in paths {
            let url = Url::parse(&format!("https://shop.example{path}")).unwrap();
            assert_eq!(
                set.is_match(url.as_str()),
                !rules.allows(&url),
                "{path} under {robots:?}: {:?}",
                deny_list.patterns
            );
        }
        assert!(deny_list.failing_closed.is_empty());
    }

    #[test]
    fn should_deny_plain_disallow_rules_as_path_prefixes() {
        assert_matches_rules(
            "User-agent: *\nDisallow: /cart\nDisallow: /*?add-to-cart=\nDisallow: /*.pdf$\n",
            &[
                "/",
                "/cart",
                "/cart/1",
                "/products/cartier-ring",
                "/shop?add-to-cart=12",
                "/shop?sort=price",
                "/a/catalogue.pdf",
                "/a/catalogue.pdf.html",
            ],
        );
    }

    #[test]
    fn should_carve_literal_allow_exceptions_out_of_a_disallow() {
        assert_matches_rules(
            "User-agent: *\nDisallow: /wp-admin/\nAllow: /wp-admin/admin-ajax.php\n",
            &[
                "/wp-admin/",
                "/wp-admin/options.php",
                "/wp-admin/admin-ajax.php",
                "/wp-admin/admin-ajax.php?action=x",
                "/wp-admin/admin-ajax.ph",
                "/wp-admin/admin-x",
                "/shop/",
            ],
        );
    }

    #[test]
    fn should_carve_an_exact_homepage_and_section_out_of_a_full_disallow() {
        assert_matches_rules(
            "User-agent: *\nDisallow: /\nAllow: /$\nAllow: /shop/\n",
            &["/", "/shop/", "/shop/item-1", "/shop", "/about", "/s"],
        );
    }

    #[test]
    fn should_deny_percent_escapes_in_either_hex_case() {
        assert_matches_rules(
            "User-agent: *\nDisallow: /collections/*%2b*\nDisallow: /a%2F\n",
            &[
                "/collections/a%2bb",
                "/collections/a%2Bb",
                "/collections/a+b",
                "/a%2f",
                "/a%2Fb",
                "/a/",
            ],
        );
    }

    #[test]
    fn should_drop_a_disallow_an_equal_allow_overrides() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /a\nAllow: /a\n");

        assert!(rules.deny_list().patterns.is_empty());
    }

    #[test]
    fn should_block_incidental_wildcard_overlaps_without_reporting_them() {
        let rules = RobotsRules::parse(
            "User-agent: *\nAllow: /*/products/account\nDisallow: /admin\nDisallow: /*?*oseid=*\n",
        );
        let deny_list = rules.deny_list();
        let set = RegexSet::new(&deny_list.patterns).unwrap();

        assert!(deny_list.failing_closed.is_empty());
        assert!(set.is_match("https://shop.example/admin/settings"));
        assert!(!set.is_match("https://shop.example/en/products/account"));
    }

    #[test]
    fn should_report_a_targeted_wildcard_exception_it_cannot_carve_out() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /*?*\nAllow: /*?page=\n");

        assert_eq!(rules.deny_list().failing_closed, vec!["/*?*".to_owned()]);
    }

    #[test]
    fn should_keep_a_disallow_in_force_when_a_wildcard_allow_overlaps_it() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /\nAllow: /*/products/\n");
        let deny_list = rules.deny_list();
        let set = RegexSet::new(&deny_list.patterns).unwrap();

        assert_eq!(deny_list.failing_closed, vec!["/".to_owned()]);
        assert!(set.is_match("https://shop.example/en/products/1"));
    }
}
