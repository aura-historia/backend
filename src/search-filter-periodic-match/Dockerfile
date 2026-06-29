FROM rust:1-bookworm AS builder

WORKDIR /app

# Build this image with the repository root as context, for example:
# docker build -f src/search-filter-periodic-match/Dockerfile .
#
# This crate inherits dependencies from the workspace root and depends on sibling
# workspace crates, so using src/search-filter-periodic-match as the build context
# cannot work.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bin search-filter-periodic-match -p search-filter-periodic-match

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/search-filter-periodic-match /usr/local/bin/search-filter-periodic-match

ENTRYPOINT ["/usr/local/bin/search-filter-periodic-match"]
