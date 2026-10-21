//! Compiles robots.txt rules into regular expressions over full URLs that
//! match the disallowed URLs, for crawl engines whose only pre-fetch veto is a
//! regex deny list.
//!
//! The `regex` crate has no lookaround, so an `Allow` exception can only be
//! carved out of a literal `Disallow` path prefix, and only when the `Allow`
//! is a literal path prefix too (it may end in `$`; a trailing `*` changes
//! nothing). Other exceptions are not
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
        // A rule matches from the path start, which is always `/`; one
        // starting with anything but `/` or `*` matches no URL.
        for disallow in self
            .rules
            .iter()
            .filter(|rule| !rule.allow && rule.pattern.starts_with(['/', '*']))
        {
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
    let disallow = literal_prefix(disallow)?;
    let (literal, exact) = match allow.strip_suffix('$') {
        Some(literal) if is_literal(literal) => (literal, true),
        Some(_) => return None,
        None => (literal_prefix(allow)?, false),
    };
    literal.strip_prefix(disallow).map(|suffix| (suffix, exact))
}

/// A pattern matching every path that starts with one literal: trailing `*`
/// add nothing to a prefix match.
fn literal_prefix(pattern: &str) -> Option<&str> {
    let literal = pattern.trim_end_matches('*');
    is_literal(literal).then_some(literal)
}

/// The regex for `disallow` minus its expressible `exceptions`, or `None`
/// when they allow everything it covers.
fn carve_exceptions(disallow: &str, exceptions: &[&Rule]) -> Option<String> {
    if exceptions.is_empty() {
        return Some(format!("{URL_ORIGIN}{}", pattern_regex(disallow)));
    }
    let Some(disallow) = literal_prefix(disallow) else {
        return Some(format!("{URL_ORIGIN}{}", pattern_regex(disallow)));
    };
    let mut allowed = AllowTrie::default();
    for exception in exceptions {
        if let Some((suffix, exact)) = literal_suffix(disallow, &exception.pattern) {
            // The exception's literal starts with the disallow's, so escapes
            // are told apart over the whole literal.
            let literal = format!("{disallow}{suffix}");
            // Only complete escapes fold case here: an exception may carve
            // out no more than robots.txt allows.
            let suffix = escape_digit_flags(&literal, false)
                .into_iter()
                .skip(disallow.chars().count());
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
    /// This node's character is a hex digit of a percent escape.
    escape_digit: bool,
    /// Every path continuing from here is allowed.
    allows_subtree: bool,
    /// A path ending exactly here is allowed.
    allows_exact: bool,
}

impl AllowTrie {
    fn insert(&mut self, suffix: impl Iterator<Item = (char, bool)>, exact: bool) {
        let mut node = self;
        for (ch, escape_digit) in suffix {
            node = node.children.entry(ch).or_insert_with(|| AllowTrie {
                escape_digit,
                ..Default::default()
            });
            // Rules disagreeing on a shared character keep it case-sensitive,
            // so the carve-out never grows past one of them.
            node.escape_digit &= escape_digit;
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
                .iter()
                .flat_map(|(&ch, child)| hex_case_variants(ch, child.escape_digit))
                .map(|ch| regex::escape(&ch.to_string()))
                .collect();
            branches.push(format!("[^{listed}]"));
        }
        for (&ch, child) in &self.children {
            if let Some(rest) = child.disallowed_suffixes() {
                branches.push(format!("{}{rest}", char_regex(ch, child.escape_digit)));
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
    let (left, right) = (literal_lead(left), literal_lead(right));
    left.starts_with(right) || right.starts_with(left)
}

fn literal_lead(pattern: &str) -> &str {
    pattern.split(['*', '$']).next().unwrap_or_default()
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
    // The path starts at the first `/`, which the host cannot contain; a
    // leading `*` before anything else needs it, or the host could match.
    let body = if pattern.trim_start_matches('*').starts_with('/') || !pattern.starts_with('*') {
        body
    } else {
        format!("/{body}")
    };
    if anchored { format!("{body}$") } else { body }
}

/// A literal pattern part as a regex. Patterns carry upper-case percent
/// escapes, while crawl engines match URLs with the escapes as written, so
/// each escape matches either hex case.
fn literal_regex(literal: &str) -> String {
    escape_digit_flags(literal, true)
        .into_iter()
        .map(|(ch, escape_digit)| char_regex(ch, escape_digit))
        .collect()
}

/// Each character of `literal`, and whether it is a hex digit of a percent
/// escape. With `cut_short`, an escape cut short by the end of the literal
/// counts too: the rule still matches a URL that completes it.
fn escape_digit_flags(literal: &str, cut_short: bool) -> Vec<(char, bool)> {
    let chars: Vec<char> = literal.chars().collect();
    let mut pending = 0_u8;
    chars
        .iter()
        .enumerate()
        .map(|(index, &ch)| {
            let escape_digit = pending > 0;
            let digits = &chars[index + 1..chars.len().min(index + 3)];
            let starts_escape = ch == '%'
                && !digits.is_empty()
                && (digits.len() == 2 || cut_short && index + 1 + digits.len() == chars.len())
                && digits.iter().all(char::is_ascii_hexdigit);
            pending = if starts_escape {
                2
            } else {
                pending.saturating_sub(1)
            };
            (ch, escape_digit)
        })
        .collect()
}

fn char_regex(ch: char, escape_digit: bool) -> String {
    match hex_case_variants(ch, escape_digit).as_slice() {
        [upper, lower] => format!("[{upper}{lower}]"),
        _ => regex::escape(&ch.to_string()),
    }
}

/// The spellings an escape's hex digit may take in a URL.
fn hex_case_variants(ch: char, escape_digit: bool) -> Vec<char> {
    if escape_digit && ch.is_ascii_alphabetic() {
        vec![ch.to_ascii_uppercase(), ch.to_ascii_lowercase()]
    } else {
        vec![ch]
    }
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
    fn should_carve_allow_exceptions_with_percent_escapes_or_a_trailing_wildcard() {
        assert_matches_rules(
            "User-agent: *\nDisallow: /a/\nAllow: /a/%2Fb\nDisallow: /wp-admin/*\nAllow: /wp-admin/admin-ajax.php*\n",
            &[
                "/a/%2Fb",
                "/a/%2fb",
                "/a/%2Fc",
                "/a/x",
                "/wp-admin/admin-ajax.php?action=x",
                "/wp-admin/options.php",
            ],
        );
    }

    #[test]
    fn should_deny_an_escape_a_rule_cuts_short_in_either_hex_case() {
        assert_matches_rules(
            "User-agent: *\nDisallow: /a%A\nDisallow: /b%C*x\n",
            &["/a%AC", "/a%aC", "/a", "/b%c1x", "/b%C1x", "/b"],
        );
    }

    #[test]
    fn should_not_carve_out_more_than_an_exception_with_a_cut_short_escape_allows() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /x\nAllow: /x%a\n");
        let set = RegexSet::new(&rules.deny_list().patterns).unwrap();

        for path in ["/x%ab", "/x%AB", "/x%aZ", "/x"] {
            let url = Url::parse(&format!("https://shop.example{path}")).unwrap();
            assert!(
                set.is_match(url.as_str()) || rules.allows(&url),
                "{path} is disallowed but not denied"
            );
        }
    }

    #[test]
    fn should_match_leading_wildcard_rules_only_against_the_path() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: *sort\nDisallow: sale\n");
        let set = RegexSet::new(&rules.deny_list().patterns).unwrap();

        for path in ["/about", "/sale", "/c?sort=price"] {
            let url = Url::parse(&format!("https://resort-sale.example{path}")).unwrap();
            assert_eq!(set.is_match(url.as_str()), !rules.allows(&url), "{path}");
        }
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
