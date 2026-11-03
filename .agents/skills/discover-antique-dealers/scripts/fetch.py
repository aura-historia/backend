#!/usr/bin/env python3
"""Fetch public dealer pages without credentials; cache evidence outside the repository."""

import argparse
import codecs
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
from http.client import HTTPException
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import tempfile
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urljoin, urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

MAX_BYTES = 3_000_000
TIMEOUT_SECONDS = 18
USER_AGENT = "Mozilla/5.0 (compatible; AuraHistoria dealer research)"
LEGAL_LINK = re.compile(r"impress|kontakt|contact|versand|liefer|shipping|about|ueber|über", re.I)
BRAND_HINT = re.compile(r"logo|brand|generator", re.I)


def request_url(url):
    parts = urlsplit(url)
    if parts.scheme not in {"http", "https"} or not parts.hostname:
        raise ValueError("Expected an absolute public HTTP(S) URL")
    if parts.username is not None or parts.password is not None:
        raise ValueError("Public-page URLs must not contain credentials")
    host = parts.hostname.encode("idna").decode("ascii")
    if ":" in host:
        host = f"[{host}]"
    if parts.port:
        host += f":{parts.port}"
    return urlunsplit((parts.scheme, host, quote(parts.path, safe="/%:@!$&'()*+,;=-._~"),
                       quote(parts.query, safe="%=&:+,/?@!$'()*;-._~"), ""))


def html_encoding(*labels):
    # HTML uses Windows-1252 for Latin-1/ASCII labels, including the euro byte 0x80.
    # https://encoding.spec.whatwg.org/
    for label in labels:
        if not label:
            continue
        try:
            encoding = codecs.lookup(label).name
        except LookupError:
            # A malformed transport label must not override valid page metadata.
            continue
        return "windows-1252" if encoding in {"iso8859-1", "ascii"} else encoding
    if not any(labels):
        return "utf-8"
    raise LookupError("No supported declared HTML encoding")


class PublicRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return super().redirect_request(request, response, code, message, headers,
                                        request_url(new_url))


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.text = []
        self.main_text = []
        self.links = []
        self.frames = []
        self.images = []
        self.base_href = None
        self.hidden = 0
        self.main_depth = 0

    def handle_starttag(self, tag, attributes):
        attrs = dict(attributes)
        if tag == "base" and attrs.get("href") and self.base_href is None:
            self.base_href = attrs["href"]
        if tag in {"script", "style"}:
            self.hidden += 1
        if tag == "main":
            self.main_depth += 1
        if tag in {"a", "area"} and attrs.get("href"):
            self.links.append(attrs["href"])
        if tag in {"frame", "iframe"} and attrs.get("src"):
            self.frames.append(attrs["src"])
        if tag == "img" or (tag == "meta" and re.search("image|logo|generator", str(attrs), re.I)):
            self.images.append(attrs)

    def handle_endtag(self, tag):
        if tag in {"script", "style"}:
            self.hidden = max(0, self.hidden - 1)
        if tag == "main":
            self.main_depth = max(0, self.main_depth - 1)

    def handle_data(self, value):
        if not self.hidden and value.strip():
            self.text.append(value.strip())
            if self.main_depth:
                self.main_text.append(value.strip())


def frame_urls(sources, base):
    urls = []
    for source in sources:
        try:
            urls.append(request_url(urljoin(base, source)))
        except ValueError:
            continue
    return list(dict.fromkeys(urls))


def parse_page(raw, url, final_url, status, encoding):
    page = Page()
    page.feed(raw)
    try:
        base = urljoin(final_url, page.base_href or "")
    except ValueError:
        base = final_url
    if urlsplit(base).scheme not in {"http", "https"}:
        base = final_url
    links = []
    for link in page.links:
        try:
            links.append(urljoin(base, link))
        except ValueError:
            # A malformed merchant link must not discard the page's evidence.
            continue
    return {
        "url": url, "finalUrl": final_url, "baseUrl": base, "status": status,
        "observedAt": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "encoding": encoding, "decodingReplacements": raw.count("\ufffd"),
        "text": "\n".join(page.text), "mainText": "\n".join(page.main_text),
        "links": list(dict.fromkeys(links)),
        "frames": frame_urls(page.frames, base),
        # Preserve original attributes: relative image paths use baseUrl, not necessarily finalUrl.
        "images": page.images,
        "technologyHints": [hint for hint in ("wp-content", "woocommerce", "cdn.shopify.com",
                                              "jimdosite.com", "cdn.website-start.de")
                            if hint in raw.lower()],
    }


def brand_previews(images):
    previews = []
    for attrs in images:
        summary = {}
        for key, value in attrs.items():
            if isinstance(value, str) and value.startswith("data:"):
                summary[key] = value.split(",", 1)[0][:80] + ";[embedded asset; see cache]"
            elif isinstance(value, str) and len(value) > 512:
                summary[key] = value[:512] + "…[see cache]"
            else:
                summary[key] = value
        if BRAND_HINT.search(str(summary)):
            previews.append(summary)
        if len(previews) == 8:
            break
    return previews


def fetch_page(url, cache_dir):
    try:
        # This client never reads API credentials or sends Authorization/Cookie headers.
        request = Request(request_url(url), headers={"User-Agent": USER_AGENT})
        with build_opener(PublicRedirect()).open(request, timeout=TIMEOUT_SECONDS) as response:
            data = response.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                raise ValueError("HTML exceeds fetch limit; qualification is incomplete")
            # A Unicode BOM takes precedence over transport and HTML declarations.
            if data.startswith(codecs.BOM_UTF8):
                encoding = "utf-8-sig"
            elif data.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)):
                encoding = "utf-16"
            else:
                declared = re.search(br"charset\s*=\s*[\"']?([A-Za-z0-9_-]+)", data[:8192], re.I)
                encoding = html_encoding(response.headers.get_content_charset(),
                                         declared.group(1).decode("ascii") if declared else None)
            raw = data.decode(encoding, errors="replace")
            result = parse_page(raw, url, response.url, response.status, encoding)
        key = hashlib.sha256(url.encode()).hexdigest()[:16]
        (cache_dir / f"{key}.html").write_text(raw, encoding="utf-8")
        (cache_dir / f"{key}.json").write_text(json.dumps(result, ensure_ascii=False), encoding="utf-8")
        preview = result["mainText"] or result["text"]
        return {
            "url": url, "file": key, "status": result["status"],
            "finalUrl": result["finalUrl"], "encoding": encoding,
            "decodingReplacements": result["decodingReplacements"],
            "previewScope": "main" if result["mainText"] else "document", "text": preview[:6000],
            "links": [link for link in result["links"] if LEGAL_LINK.search(link)],
            "frames": result["frames"][:12],
            "images": brand_previews(result["images"]),
            "technologyHints": result["technologyHints"],
        }
    except HTTPError as error:
        return {"url": url, "error": f"HTTP {error.code}"}
    except (URLError, HTTPException, ValueError, LookupError, OSError) as error:
        return {"url": url, "error": f"{type(error).__name__}: {str(error)[:400]}"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("urls", nargs="+")
    parser.add_argument("--cache-dir", type=Path,
                        default=Path(tempfile.gettempdir()) / "aura-dealer-discovery" / "pages")
    parser.add_argument("--workers", type=int, choices=range(1, 9), default=4)
    args = parser.parse_args()
    for url in args.urls:
        try:
            request_url(url)
        except ValueError as error:
            parser.error(str(error))
    args.cache_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        for result in pool.map(lambda url: fetch_page(url, args.cache_dir), args.urls):
            print(json.dumps(result, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
