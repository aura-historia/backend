//! The top-level schema.org `Product` nodes of a JSON-LD document, read without fetching any
//! remote context: what a node's `@context` makes of the type names `Product` and
//! `prefix:Product`.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The top-level schema.org `Product` nodes of one JSON-LD document: the document itself, the
/// members of an array and of a node's `@graph`, each typed `Product` under its active context.
/// The `json_ld_product` extraction kind reads these nodes, and the review console's product
/// markup check counts them, so both see the same products.
pub(crate) fn top_level_products(value: &Value) -> Vec<&Map<String, Value>> {
    let mut products = Vec::new();
    collect_top_level_products(value, &ActiveContext::initial(), &mut products);
    products
}

/// Collects the `Product` nodes of a JSON-LD value at its top level: the value itself, the members
/// of an array and the members of a node's `@graph`, never a node's other properties. A node's own
/// `@context` applies to its `@type` and its `@graph`, on top of the `context` it inherits.
fn collect_top_level_products<'a>(
    value: &'a Value,
    context: &ActiveContext,
    products: &mut Vec<&'a Map<String, Value>>,
) {
    match value {
        Value::Array(nodes) => {
            for node in nodes {
                collect_top_level_products(node, context, products);
            }
        }
        Value::Object(node) => {
            let context = context.of_node(node);
            if node
                .get("@type")
                .is_some_and(|types| context.names_product_in(types))
            {
                products.push(node);
            }
            if let Some(graph) = node.get("@graph") {
                collect_top_level_products(graph, &context, products);
            }
        }
        _ => {}
    }
}

/// What a JSON-LD node's `@context` makes of a type name, read without fetching anything: whether
/// bare terms are schema.org's (its `@vocab`), which terms it defines, and which prefixes are bound
/// to schema.org's vocabulary. A node without any context reads bare terms as schema.org's, as
/// pages that omit the context mean schema.org; a context it cannot read, such as another remote
/// context, reads none, so a node of another vocabulary never counts.
#[derive(Clone, Debug)]
pub(crate) struct ActiveContext {
    bare_terms_are_schema_org: bool,
    /// Terms the context defines, each with the schema.org term it maps to, `None` for a term of
    /// another vocabulary. A definition outranks `bare_terms_are_schema_org` until a later
    /// `@vocab` replaces both, as the context's `Product` definition always did.
    defined_terms: BTreeMap<String, Option<String>>,
    schema_org_prefixes: BTreeSet<String>,
}

impl ActiveContext {
    pub(crate) fn initial() -> Self {
        Self {
            bare_terms_are_schema_org: true,
            defined_terms: BTreeMap::new(),
            schema_org_prefixes: BTreeSet::new(),
        }
    }

    fn unreadable() -> Self {
        Self {
            bare_terms_are_schema_org: false,
            defined_terms: BTreeMap::new(),
            schema_org_prefixes: BTreeSet::new(),
        }
    }

    /// The context of a node: this one with the node's own `@context` processed on top, or this
    /// one when the node has none.
    pub(crate) fn of_node(&self, node: &Map<String, Value>) -> Self {
        node.get("@context")
            .map_or_else(|| self.clone(), |local| self.with(local))
    }

    /// This context with `context` processed on top of it: `null` resets it, schema.org's own
    /// context makes bare terms and `schema:` terms schema.org's, an array applies each entry in
    /// order, and an object applies its `@vocab`, its prefix definitions and its term definitions.
    fn with(&self, context: &Value) -> Self {
        match context {
            Value::Null => Self::initial(),
            Value::String(iri) if is_schema_org_context(iri) => Self {
                bare_terms_are_schema_org: true,
                defined_terms: BTreeMap::new(),
                schema_org_prefixes: BTreeSet::from(["schema".to_owned()]),
            },
            Value::Array(contexts) => contexts
                .iter()
                .fold(self.clone(), |active, context| active.with(context)),
            Value::Object(definitions) => self.with_definitions(definitions),
            _ => Self::unreadable(),
        }
    }

    fn with_definitions(&self, definitions: &Map<String, Value>) -> Self {
        let mut active = self.clone();
        if let Some(vocab) = definitions.get("@vocab") {
            active.bare_terms_are_schema_org = vocab.as_str().is_some_and(is_schema_org_vocabulary);
            active.defined_terms.clear();
        }
        for (term, definition) in definitions
            .iter()
            .filter(|(term, _)| !term.starts_with('@'))
        {
            let iri = match definition {
                Value::Object(expanded) => expanded.get("@id").and_then(Value::as_str),
                other => other.as_str(),
            };
            active
                .defined_terms
                .insert(term.clone(), iri.and_then(|iri| self.schema_org_term(iri)));
            if term == "Product" {
                continue;
            }
            if iri.is_some_and(is_schema_org_vocabulary) {
                active.schema_org_prefixes.insert(term.clone());
            } else {
                active.schema_org_prefixes.remove(term);
            }
        }
        active
    }

    /// The schema.org terms a node's `@type`, a name or an array of names, names under this
    /// context; names of other vocabularies are left out.
    pub(crate) fn schema_org_types(&self, types: &Value) -> Vec<String> {
        match types {
            Value::String(name) => self.schema_org_term(name).into_iter().collect(),
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|name| self.schema_org_term(name))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether a node's `@type`, a name or an array of names, includes schema.org's `Product`.
    fn names_product_in(&self, types: &Value) -> bool {
        self.schema_org_types(types)
            .iter()
            .any(|term| term == "Product")
    }

    /// The schema.org term `name` names: an IRI in schema.org's vocabulary, or a bare term or a
    /// compact `prefix:term` this context maps to it.
    fn schema_org_term(&self, name: &str) -> Option<String> {
        let name = name.trim();
        if let Some(term) = schema_org_term_of_iri(name) {
            return Some(term.to_owned());
        }
        if !name.contains(':') {
            // A definition only confirms or denies that a bare term is schema.org's own term of
            // that name; an alias for another term is not followed.
            return match self.defined_terms.get(name) {
                Some(defined) => defined.clone().filter(|term| term == name),
                None => self.bare_terms_are_schema_org.then(|| name.to_owned()),
            };
        }
        name.split_once(':')
            .filter(|(prefix, term)| {
                self.schema_org_prefixes.contains(*prefix) && is_schema_org_term(term)
            })
            .map(|(_, term)| term.to_owned())
    }
}

/// schema.org's own address of `Product`: `http` or `https`, the host `schema.org` or
/// `www.schema.org`, and the path `/Product`, with or without one trailing slash. schema.org
/// publishes `https://schema.org/Product`; the other forms are spellings pages use for the same
/// term. Scheme and host match in any case, as URLs do; the term is case-sensitive, as schema.org's
/// terms are. Anything else, such as another host ending in `schema.org` or a path ending in
/// `schema.org/Product`, names another vocabulary's type.
pub(crate) fn is_product_iri(name: &str) -> bool {
    schema_org_term_of_iri(name) == Some("Product")
}

/// The schema.org term an IRI in schema.org's vocabulary names, in the spellings
/// [`is_product_iri`] describes: `https://schema.org/ProductGroup` names `ProductGroup`. `None` for
/// any other address, or a path that is no single term.
pub(crate) fn schema_org_term_of_iri(name: &str) -> Option<&str> {
    let path = schema_org_path(name)?;
    let term = path.strip_suffix('/').unwrap_or(path);
    is_schema_org_term(term).then_some(term)
}

/// A schema.org term: a non-empty run of ASCII letters and digits, such as `Product`.
fn is_schema_org_term(term: &str) -> bool {
    !term.is_empty() && term.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// schema.org's vocabulary address, `https://schema.org/` in any of the spellings
/// [`is_product_iri`] accepts, with or without the trailing slash.
fn is_schema_org_vocabulary(iri: &str) -> bool {
    schema_org_path(iri.trim()) == Some("")
}

/// A JSON-LD context that is schema.org's own: its vocabulary address, which serves the context,
/// or the context document it publishes.
fn is_schema_org_context(iri: &str) -> bool {
    matches!(
        schema_org_path(iri.trim()),
        Some("" | "docs/jsonldcontext.json" | "docs/jsonldcontext.jsonld")
    )
}

/// The path of an `http` or `https` address on `schema.org` or `www.schema.org`, without its
/// leading slash; `None` for any other address.
fn schema_org_path(iri: &str) -> Option<&str> {
    let (scheme, address) = iri.split_once("://")?;
    let (host, path) = address.split_once('/').unwrap_or((address, ""));
    let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let schema_org =
        host.eq_ignore_ascii_case("schema.org") || host.eq_ignore_ascii_case("www.schema.org");
    (web && schema_org).then_some(path)
}
