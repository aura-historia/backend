//! A `state` rule that reads page chrome: a form button, the site navigation, a shop-wide notice,
//! a cookie banner or a marketing line. Each such value is an ordinary word or sentence that no
//! per-page check can tell from a status label, and the ProductListing normalization leaves it
//! unresolved, so the listings never get an availability. What gives it away is that it never
//! varies: the review pages show different products, a sold one among them, so a per-listing
//! status differs between them, while chrome reads the same on every page.
//!
//! A constant value that resolves to an availability, such as "Sold" on every page of a sold
//! archive or an "Add to cart" action on every live page, is no finding: the listings get an
//! availability from it.

use super::super::resolves_to_availability;
use super::collapse_whitespace;

/// For each page of `states`, its `state` when every page extracts the same one, compared with
/// whitespace runs collapsed, and the normalization resolves it to no availability; empty
/// otherwise. It takes at least two pages, as every check that compares pages does, and a page
/// that extracts no `state` breaks the run, since a rule that misses on one product's page reads
/// something that differs per page.
pub(super) fn constant_unresolved_states(states: &[Option<&str>]) -> Vec<Vec<String>> {
    let states: Vec<Option<&str>> = states
        .iter()
        .map(|state| state.map(str::trim).filter(|state| !state.is_empty()))
        .collect();
    let keys: Option<Vec<String>> = states
        .iter()
        .map(|state| state.map(collapse_whitespace))
        .collect();
    let is_constant_chrome = keys.is_some_and(|keys| match keys.as_slice() {
        [first, rest @ ..] if !rest.is_empty() => {
            rest.iter().all(|key| key == first) && !resolves_to_availability(first)
        }
        _ => false,
    });
    states
        .into_iter()
        .map(|state| {
            state
                .filter(|_| is_constant_chrome)
                .map(str::to_owned)
                .into_iter()
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(states: &[Option<&str>]) -> Vec<Vec<String>> {
        constant_unresolved_states(states)
    }

    fn each(value: &str, pages: usize) -> Vec<Vec<String>> {
        vec![vec![value.to_owned()]; pages]
    }

    /// The live `state` values of the shops #2878 names: a form button, the site navigation, a
    /// shop-wide notice and a marketing line, each on every page of the shop.
    #[rstest::rstest]
    #[case::form_button("Valider")]
    #[case::navigation("Acheter un livre ancien")]
    #[case::shop_wide_notice("欲購買任何商品請私訊我們的粉絲專頁")]
    #[case::marketing_line("Acheter en toute sécurité avec Stripe")]
    #[case::cookie_banner("Cookies erlauben")]
    fn should_name_a_state_every_page_shares_that_resolves_to_no_availability(
        #[case] chrome: &str,
    ) {
        assert_eq!(named(&[Some(chrome); 4]), each(chrome, 4));
    }

    #[test]
    fn should_compare_trimmed_states_with_whitespace_runs_collapsed() {
        let states = [
            Some("Message  us to buy"),
            Some("Message us\nto buy"),
            Some(" Message us to buy "),
        ];

        assert_eq!(
            named(&states),
            vec![
                vec!["Message  us to buy".to_owned()],
                vec!["Message us\nto buy".to_owned()],
                vec!["Message us to buy".to_owned()],
            ]
        );
    }

    #[test]
    fn should_not_name_states_that_differ_between_pages() {
        assert_eq!(
            named(&[Some("Valider"), Some("Valider"), Some("Vendu")]),
            vec![Vec::<String>::new(); 3]
        );
    }

    /// A sold archive shows "Sold" on every page, and every live page of a shop may offer the
    /// same cart action: the listings get an availability from either.
    #[rstest::rstest]
    #[case::sold("Sold")]
    #[case::cart_action("Add to cart")]
    #[case::schema_org("https://schema.org/InStock")]
    fn should_not_name_a_shared_state_that_resolves_to_an_availability(#[case] status: &str) {
        assert_eq!(named(&[Some(status); 3]), vec![Vec::<String>::new(); 3]);
    }

    #[test]
    fn should_not_name_the_state_of_a_single_page() {
        assert_eq!(named(&[Some("Valider")]), vec![Vec::<String>::new()]);
        assert_eq!(named(&[]), Vec::<Vec<String>>::new());
    }

    #[test]
    fn should_not_name_a_shared_state_when_a_page_extracts_none() {
        assert_eq!(
            named(&[Some("Valider"), None, Some("Valider")]),
            vec![Vec::<String>::new(); 3]
        );
        assert_eq!(
            named(&[Some("Valider"), Some("  "), Some("Valider")]),
            vec![Vec::<String>::new(); 3]
        );
    }
}
