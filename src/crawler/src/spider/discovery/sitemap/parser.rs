//! A streaming reader of one sitemap file (<https://www.sitemaps.org/protocol.html>): a URL set
//! or a sitemap index, read event by event without building a document, so a file is read in one
//! pass and stops at the entry limit (`docs/crawler/networking.md#sitemaps`).
//!
//! A sitemap is untrusted input and is read fail-closed: a document whose root is not
//! `<urlset>` or `<sitemapindex>`, such as an RSS or Atom feed or an HTML page a shop advertises
//! as its sitemap, is no sitemap; a document that is not well-formed XML is malformed, and none
//! of its entries are used. Only the `<loc>` of each entry is read; `<lastmod>`, `<priority>`,
//! `<changefreq>` and extensions such as image sitemaps are skipped, so a bad date never rejects
//! an entry. An entry whose location cannot be read, such as one with an entity the XML
//! standard does not define, is left out and counted. Entities are never expanded from a DTD.

use quick_xml::events::{BytesRef, Event};
use quick_xml::reader::Reader;

/// The longest location the sitemap protocol allows; a longer one is left out.
const MAX_LOCATION_CHARS: usize = 2048;

/// What a sitemap file lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitemapKind {
    /// `<urlset>`: the pages of the shop.
    UrlSet,
    /// `<sitemapindex>`: further sitemap files.
    Index,
}

/// The entries of one sitemap file, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSitemap {
    pub kind: SitemapKind,
    /// The `<loc>` of each entry, trimmed, at most the entry limit.
    pub locations: Vec<String>,
    /// The file lists more entries than the limit; reading stopped there.
    pub truncated: bool,
    /// Entries without a readable `<loc>`, which were left out.
    pub invalid_entries: usize,
}

/// Why a body is no sitemap the crawler reads.
#[derive(Debug, thiserror::Error)]
pub enum SitemapParseError {
    /// The document's root is not a sitemap's, such as `rss`, `feed` or `html`.
    #[error("document root <{root}> is not a sitemap")]
    NotASitemap { root: String },
    /// The body holds no XML element, such as a plain-text page.
    #[error("body holds no XML document")]
    NoDocument,
    #[error("sitemap is not well-formed XML")]
    Malformed(#[source] quick_xml::Error),
    /// The document ended before its root element was closed.
    #[error("sitemap ends before its root element is closed")]
    Unterminated,
}

impl SitemapParseError {
    /// Stable label for the crawl diagnostics; not persisted.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotASitemap { .. } | Self::NoDocument => "not_a_sitemap",
            Self::Malformed(_) | Self::Unterminated => "malformed",
        }
    }
}

/// Reads the sitemap `body`, keeping at most `max_entries` locations.
pub fn parse_sitemap(body: &[u8], max_entries: usize) -> Result<ParsedSitemap, SitemapParseError> {
    let mut reader = Reader::from_reader(body);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.expand_empty_elements = false;
    let kind = read_root(&mut reader)?;
    let mut sitemap = ParsedSitemap {
        kind,
        locations: Vec::new(),
        truncated: false,
        invalid_entries: 0,
    };
    let mut entries = EntryReader::new(kind);
    loop {
        match reader.read_event().map_err(SitemapParseError::Malformed)? {
            Event::Start(start) => entries.open(start.local_name().as_ref()),
            Event::End(_) => match entries.close() {
                Closed::Root => return Ok(sitemap),
                Closed::Entry(Some(_)) if sitemap.locations.len() >= max_entries => {
                    sitemap.truncated = true;
                    return Ok(sitemap);
                }
                Closed::Entry(Some(location)) => sitemap.locations.push(location),
                Closed::Entry(None) => sitemap.invalid_entries += 1,
                Closed::Other => {}
            },
            Event::Empty(empty) => {
                if entries.is_entry(empty.local_name().as_ref()) {
                    sitemap.invalid_entries += 1;
                }
            }
            Event::Text(text) => entries.text(text.xml10_content().ok().as_deref()),
            Event::CData(data) => entries.text(data.decode().ok().as_deref()),
            Event::GeneralRef(reference) => entries.text(resolve_reference(&reference).as_deref()),
            Event::Eof => return Err(SitemapParseError::Unterminated),
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
}

/// Reads up to the document's root element and tells which sitemap it opens.
fn read_root(reader: &mut Reader<&[u8]>) -> Result<SitemapKind, SitemapParseError> {
    loop {
        match reader.read_event().map_err(SitemapParseError::Malformed)? {
            Event::Start(root) => {
                return match root.local_name().as_ref() {
                    b"urlset" => Ok(SitemapKind::UrlSet),
                    b"sitemapindex" => Ok(SitemapKind::Index),
                    other => Err(SitemapParseError::NotASitemap {
                        root: loggable_name(other),
                    }),
                };
            }
            // An empty root lists nothing, whatever it is named.
            Event::Empty(root) => {
                return Err(SitemapParseError::NotASitemap {
                    root: loggable_name(root.local_name().as_ref()),
                });
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) | Event::Eof => {
                return Err(SitemapParseError::NoDocument);
            }
            Event::End(_) => return Err(SitemapParseError::NoDocument),
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
}

/// An element name fit for the crawl diagnostics: short ASCII names as written, anything else
/// replaced, since the name comes from an untrusted document.
fn loggable_name(name: &[u8]) -> String {
    let readable = name.len() <= 32
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    match readable {
        true => String::from_utf8_lossy(name).into_owned(),
        false => "unreadable".to_owned(),
    }
}

/// The character an entity or character reference stands for: the five entities XML predefines
/// and numeric references. Any other entity would come from a DTD, which is never read.
fn resolve_reference(reference: &BytesRef<'_>) -> Option<String> {
    if reference.is_char_ref() {
        return reference
            .resolve_char_ref()
            .ok()
            .flatten()
            .map(String::from);
    }
    let resolved = match reference.as_ref() {
        b"amp" => "&",
        b"lt" => "<",
        b"gt" => ">",
        b"quot" => "\"",
        b"apos" => "'",
        _ => return None,
    };
    Some(resolved.to_owned())
}

/// What an end tag closed.
enum Closed {
    Root,
    /// An entry with its location, or `None` when it had none that could be read.
    Entry(Option<String>),
    Other,
}

/// Tracks where the reader is below the root: an entry (`<url>` or `<sitemap>`) directly below
/// it, and that entry's `<loc>` directly below the entry. Elements anywhere else are skipped.
struct EntryReader {
    entry_name: &'static [u8],
    /// Open elements below the root, the root not counted.
    depth: usize,
    in_entry: bool,
    in_location: bool,
    location: LocationText,
    /// The entry's first `<loc>`, once closed.
    entry_location: Option<LocationText>,
}

impl EntryReader {
    fn new(kind: SitemapKind) -> Self {
        Self {
            entry_name: match kind {
                SitemapKind::UrlSet => b"url",
                SitemapKind::Index => b"sitemap",
            },
            depth: 0,
            in_entry: false,
            in_location: false,
            location: LocationText::default(),
            entry_location: None,
        }
    }

    fn is_entry(&self, name: &[u8]) -> bool {
        self.depth == 0 && name == self.entry_name
    }

    fn open(&mut self, name: &[u8]) {
        if self.is_entry(name) {
            self.in_entry = true;
            self.entry_location = None;
        } else if self.in_entry
            && self.depth == 1
            && name == b"loc"
            && self.entry_location.is_none()
        {
            self.in_location = true;
            self.location = LocationText::default();
        }
        self.depth += 1;
    }

    fn close(&mut self) -> Closed {
        if self.depth == 0 {
            return Closed::Root;
        }
        self.depth -= 1;
        if self.in_location && self.depth == 1 {
            self.in_location = false;
            self.entry_location = Some(std::mem::take(&mut self.location));
            return Closed::Other;
        }
        if self.in_entry && self.depth == 0 {
            self.in_entry = false;
            return Closed::Entry(
                self.entry_location
                    .take()
                    .and_then(LocationText::into_location),
            );
        }
        Closed::Other
    }

    /// Adds text to the open `<loc>`; `None` is text that could not be read, which spoils it.
    fn text(&mut self, text: Option<&str>) {
        // Text of a nested element inside `<loc>` is no part of the location.
        if self.in_location && self.depth == 2 {
            self.location.push(text);
        }
    }
}

#[derive(Debug, Default)]
struct LocationText {
    text: String,
    unreadable: bool,
}

impl LocationText {
    fn push(&mut self, text: Option<&str>) {
        match text {
            Some(text) if self.text.len() <= MAX_LOCATION_CHARS => self.text.push_str(text),
            Some(_) => {}
            None => self.unreadable = true,
        }
    }

    fn into_location(self) -> Option<String> {
        let location = self.text.trim();
        let usable = !self.unreadable
            && !location.is_empty()
            && location.chars().count() <= MAX_LOCATION_CHARS;
        usable.then(|| location.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<ParsedSitemap, SitemapParseError> {
        parse_sitemap(body.as_bytes(), 100)
    }

    #[test]
    fn should_read_the_locations_of_a_url_set_in_document_order() {
        let sitemap = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
                    xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
              <url>
                <loc> https://shop.example/products/a </loc>
                <lastmod>not a date</lastmod>
                <image:image><image:loc>https://cdn.example/a.jpg</image:loc></image:image>
              </url>
              <url><loc>https://shop.example/products/b?x=1&amp;y=2</loc></url>
            </urlset>"#,
        )
        .unwrap();

        assert_eq!(sitemap.kind, SitemapKind::UrlSet);
        assert_eq!(
            sitemap.locations,
            [
                "https://shop.example/products/a",
                "https://shop.example/products/b?x=1&y=2"
            ]
        );
        assert!(!sitemap.truncated);
        assert_eq!(sitemap.invalid_entries, 0);
    }

    #[test]
    fn should_read_the_child_sitemaps_of_an_index_with_a_namespace_prefix() {
        let sitemap = parse(
            r#"<sm:sitemapindex xmlns:sm="http://www.sitemaps.org/schemas/sitemap/0.9">
              <sm:sitemap><sm:loc><![CDATA[https://shop.example/sitemap_products_1.xml?from=1&to=9]]></sm:loc></sm:sitemap>
              <sm:sitemap><sm:loc>https://shop.example/sitemap_pages_1.xml</sm:loc></sm:sitemap>
            </sm:sitemapindex>"#,
        )
        .unwrap();

        assert_eq!(sitemap.kind, SitemapKind::Index);
        assert_eq!(
            sitemap.locations,
            [
                "https://shop.example/sitemap_products_1.xml?from=1&to=9",
                "https://shop.example/sitemap_pages_1.xml"
            ]
        );
    }

    #[test]
    fn should_stop_at_the_entry_limit_and_report_the_file_truncated() {
        let body = format!(
            "<urlset>{}</urlset>",
            (0..5)
                .map(|n| format!("<url><loc>https://shop.example/p/{n}</loc></url>"))
                .collect::<String>()
        );

        let sitemap = parse_sitemap(body.as_bytes(), 3).unwrap();

        assert_eq!(sitemap.locations.len(), 3);
        assert!(sitemap.truncated);
    }

    #[test]
    fn should_leave_out_entries_without_a_readable_location() {
        let sitemap = parse(
            "<urlset>
              <url><lastmod>2024-01-01</lastmod></url>
              <url><loc>   </loc></url>
              <url><loc>https://shop.example/&custom;</loc></url>
              <url/>
              <url><loc>https://shop.example/&#x6B;ept</loc></url>
            </urlset>",
        )
        .unwrap();

        assert_eq!(sitemap.locations, ["https://shop.example/kept"]);
        assert_eq!(sitemap.invalid_entries, 4);
    }

    #[test]
    fn should_read_only_the_first_location_of_an_entry_and_none_outside_entries() {
        let sitemap = parse(
            "<urlset>
              <loc>https://shop.example/stray</loc>
              <url><loc>https://shop.example/first</loc><loc>https://shop.example/second</loc></url>
            </urlset>",
        )
        .unwrap();

        assert_eq!(sitemap.locations, ["https://shop.example/first"]);
    }

    #[test]
    fn should_refuse_a_feed_or_page_advertised_as_a_sitemap() {
        for (body, root) in [
            (
                "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><item><link>https://shop.example/a</link></item></channel></rss>",
                "rss",
            ),
            (
                "<feed xmlns=\"http://www.w3.org/2005/Atom\"></feed>",
                "feed",
            ),
            ("<!DOCTYPE html><html><body>Not found</body></html>", "html"),
            ("<urlset/>", "urlset"),
        ] {
            let error = parse(body).unwrap_err();

            assert!(
                matches!(&error, SitemapParseError::NotASitemap { root: found } if found == root),
                "{body}: {error:?}"
            );
            assert_eq!(error.as_str(), "not_a_sitemap");
        }
    }

    #[test]
    fn should_refuse_a_body_without_an_xml_document() {
        for body in ["", "https://shop.example/a\nhttps://shop.example/b\n", "  "] {
            assert!(
                matches!(parse(body), Err(SitemapParseError::NoDocument)),
                "{body:?}"
            );
        }
    }

    #[test]
    fn should_use_no_entry_of_a_malformed_or_cut_off_sitemap() {
        for body in [
            "<urlset><url><loc>https://shop.example/a</loc></url></sitemapindex>",
            "<urlset><url><loc>https://shop.example/a</loc></url>",
            "<urlset><url><loc>https://shop.example/a</loc></url><url><loc>https://shop",
        ] {
            let error = parse(body).unwrap_err();

            assert_eq!(error.as_str(), "malformed", "{body}: {error:?}");
        }
    }

    #[test]
    fn should_not_expand_entities_a_document_type_declares() {
        let sitemap = parse(
            r#"<!DOCTYPE urlset [<!ENTITY host "internal.example">]>
            <urlset><url><loc>https://&host;/a</loc></url></urlset>"#,
        )
        .unwrap();

        assert!(sitemap.locations.is_empty());
        assert_eq!(sitemap.invalid_entries, 1);
    }

    #[test]
    fn should_leave_out_a_location_longer_than_the_protocol_allows() {
        let long = format!("https://shop.example/{}", "a".repeat(MAX_LOCATION_CHARS));
        let body = format!("<urlset><url><loc>{long}</loc></url></urlset>");

        let sitemap = parse(&body).unwrap();

        assert!(sitemap.locations.is_empty());
        assert_eq!(sitemap.invalid_entries, 1);
    }

    #[test]
    fn should_name_an_unreadable_root_without_echoing_it() {
        let error = parse("<a-very-long-root-element-name-that-goes-on-and-on/>").unwrap_err();

        assert!(matches!(error, SitemapParseError::NotASitemap { root } if root == "unreadable"));
    }
}
