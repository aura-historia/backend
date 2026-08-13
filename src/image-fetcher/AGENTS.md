# DOX

## Purpose

- Own reusable safe external image retrieval.

## Core Design

- `ImageFetcher` resolves and validates every HTTP(S) target and redirect before request.
- Fetches are bounded by retries, time, redirects, and response bytes.
- Returned `FetchedImage` contains sniffed image media type and base64 bytes.
- Fetch failure means no image. Callers decide whether an image is optional or required.

## Ownership

- This doc rules `src/image-fetcher/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Never weaken SSRF, redirect, size, or media validation.
- Do not log image URL or payload.

## Verification

- `cargo check -p image-fetcher`
- `cargo test -p image-fetcher --all-features`
