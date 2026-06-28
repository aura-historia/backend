FROM rust:1-bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --locked --release --bin search-filter-periodic-match

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/search-filter-periodic-match /usr/local/bin/search-filter-periodic-match

ENTRYPOINT ["/usr/local/bin/search-filter-periodic-match"]
