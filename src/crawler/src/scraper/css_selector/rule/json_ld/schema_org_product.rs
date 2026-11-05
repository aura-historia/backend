//! The top-level schema.org `Product` nodes of a JSON-LD document, read without fetching any
//! remote context: what a node's `@context` makes of the type names `Product` and
//! `prefix:Product`.

use serde_json::{Map, Value};
use std::collections::BTreeSet;

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
            let context = node
                .get("@context")
                .map_or_else(|| context.clone(), |local| context.with(local));
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

/// What a JSON-LD node's `@context` makes of the type names `Product` and `prefix:Product`, read
/// without fetching anything: whether the bare term is schema.org's `Product`, and which prefixes
/// are bound to schema.org's vocabulary. A node without any context counts the bare term, as pages
/// that omit the context mean schema.org; a context it cannot read, such as another remote
/// context, counts neither, so a node of another vocabulary never counts.
#[derive(Clone, Debug)]
struct ActiveContext {
    bare_product: bool,
    schema_org_prefixes: BTreeSet<String>,
}

impl ActiveContext {
    fn initial() -> Self {
        Self {
            bare_product: true,
            schema_org_prefixes: BTreeSet::new(),
        }
    }

    fn unreadable() -> Self {
        Self {
            bare_product: false,
            schema_org_prefixes: BTreeSet::new(),
        }
    }

    /// This context with `context` processed on top of it: `null` resets it, schema.org's own
    /// context makes `Product` and `schema:Product` schema.org's, an array applies each entry in
    /// order, and an object applies its `@vocab`, its prefix definitions and a definition of the
    /// term `Product`.
    fn with(&self, context: &Value) -> Self {
        match context {
            Value::Null => Self::initial(),
            Value::String(iri) if is_schema_org_context(iri) => Self {
                bare_product: true,
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
            active.bare_product = vocab.as_str().is_some_and(is_schema_org_vocabulary);
        }
        for (term, definition) in definitions
            .iter()
            .filter(|(term, _)| !term.starts_with('@'))
        {
            let iri = match definition {
                Value::Object(expanded) => expanded.get("@id").and_then(Value::as_str),
                other => other.as_str(),
            };
            if term == "Product" {
                active.bare_product = iri.is_some_and(|iri| self.names_product(iri));
            } else if iri.is_some_and(is_schema_org_vocabulary) {
                active.schema_org_prefixes.insert(term.clone());
            } else {
                active.schema_org_prefixes.remove(term);
            }
        }
        active
    }

    /// Whether a node's `@type`, a name or an array of names, includes schema.org's `Product`.
    fn names_product_in(&self, types: &Value) -> bool {
        match types {
            Value::String(name) => self.names_product(name),
            Value::Array(names) => names
                .iter()
                .any(|name| name.as_str().is_some_and(|name| self.names_product(name))),
            _ => false,
        }
    }

    /// `Product` in schema.org's vocabulary: its IRI, or the bare term or a compact
    /// `prefix:Product` this context maps to it.
    fn names_product(&self, name: &str) -> bool {
        let name = name.trim();
        if is_product_iri(name) {
            return true;
        }
        if name == "Product" {
            return self.bare_product;
        }
        name.split_once(':').is_some_and(|(prefix, term)| {
            term == "Product" && self.schema_org_prefixes.contains(prefix)
        })
    }
}

/// schema.org's own address of `Product`: `http` or `https`, the host `schema.org` or
/// `www.schema.org`, and the path `/Product`, with or without one trailing slash. schema.org
/// publishes `https://schema.org/Product`; the other forms are spellings pages use for the same
/// term. Scheme and host match in any case, as URLs do; the term is case-sensitive, as schema.org's
/// terms are. Anything else, such as another host ending in `schema.org` or a path ending in
/// `schema.org/Product`, names another vocabulary's type.
pub(crate) fn is_product_iri(name: &str) -> bool {
    matches!(schema_org_path(name), Some("Product" | "Product/"))
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
