FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS source

# Signed, fixed Debian snapshot keeps native dependency resolution stable too.
RUN rm /etc/apt/sources.list.d/debian.sources \
    && printf '%s\n' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260901T000000Z bookworm main' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20260901T000000Z bookworm-security main' \
        > /etc/apt/sources.list \
    && apt-get update \
    && apt-get install --yes --no-install-recommends cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Use the installed pin; do not download the repo's editor-only components.
ENV RUSTUP_TOOLCHAIN=1.98.0 \
    CARGO_BUILD_JOBS=2
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo/config.toml .cargo/config.toml
COPY src/ src/
COPY src/test-api/postgres/image-ref.txt src/test-api/postgres/image-ref.txt
COPY migrations/ migrations/
COPY opensearch/ opensearch/

FROM source AS build

ARG COMMIT_SHA
RUN test "${#COMMIT_SHA}" -eq 40 \
    && printf '%s\n' "${COMMIT_SHA}" | grep -Eq '^[0-9a-f]{40}$' \
    && ! printf '%s\n' "${COMMIT_SHA}" | grep -Eq '^([0-9a-f])\1{39}$|^0123456789abcdef' \
    && cargo build --release --locked \
        -p aura-historia-api --bin aura-historia-api \
        -p aura-historia-worker --bin aura-historia-worker \
        -p aura-historia-cron --bin aura-historia-cron \
        -p crawler --bin server

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime
RUN rm /etc/apt/sources.list.d/debian.sources \
    && printf '%s\n' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260901T000000Z bookworm main' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20260901T000000Z bookworm-security main' \
        > /etc/apt/sources.list \
    && apt-get update \
    && apt-get install --yes --no-install-recommends \
        ca-certificates libssl3 libgcc-s1 libstdc++6 zlib1g \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 aura \
    && useradd --uid 10001 --gid 10001 --create-home --shell /usr/sbin/nologin aura
ARG COMMIT_SHA
ENV COMMIT_SHA=${COMMIT_SHA} \
    HOME=/home/aura
LABEL org.opencontainers.image.source="https://github.com/aura-historia/backend" \
      org.opencontainers.image.revision="${COMMIT_SHA}"
WORKDIR /home/aura
USER 10001:10001
STOPSIGNAL SIGTERM

FROM runtime AS api
COPY --from=build /app/target/release/aura-historia-api /usr/local/bin/aura-historia-api
ENTRYPOINT ["/usr/local/bin/aura-historia-api"]

FROM runtime AS worker
COPY --from=build /app/target/release/aura-historia-worker /usr/local/bin/aura-historia-worker
ENTRYPOINT ["/usr/local/bin/aura-historia-worker"]

FROM runtime AS cron
COPY --from=build /app/target/release/aura-historia-cron /usr/local/bin/aura-historia-cron
ENTRYPOINT ["/usr/local/bin/aura-historia-cron"]

FROM runtime AS crawler
COPY --from=build /app/target/release/server /usr/local/bin/server
ENTRYPOINT ["/usr/local/bin/server"]

# Python-only secured-engine test helper. No dependency on build/smoke-build.
# Never published or deployed as an application image.
FROM runtime AS opensearch-test-helper
USER root
RUN apt-get update \
    && apt-get install --yes --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
USER 10001:10001
ENTRYPOINT ["/usr/bin/python3"]

# Test executables only. This intentionally does not inherit the production build
# stage, which compiles four release binaries.
FROM source AS opensearch-tls-witness-build
RUN cargo test --locked --no-run --all-features -p aura-historia-api --lib \
    && cargo test --locked --no-run --all-features -p aura-historia-worker --bin aura-historia-worker \
    && cargo test --locked --no-run --all-features -p aura-historia-cron --lib \
    && api_test="$(find target/debug/deps -maxdepth 1 -type f -executable -name 'aura_historia_api-*' -print)" \
    && worker_test="$(find target/debug/deps -maxdepth 1 -type f -executable -name 'aura_historia_worker-*' -print)" \
    && cron_test="$(find target/debug/deps -maxdepth 1 -type f -executable -name 'aura_historia_cron-*' -print)" \
    && test "$(printf '%s\n' "$api_test" | sed '/^$/d' | wc -l)" -eq 1 \
    && test "$(printf '%s\n' "$worker_test" | sed '/^$/d' | wc -l)" -eq 1 \
    && test "$(printf '%s\n' "$cron_test" | sed '/^$/d' | wc -l)" -eq 1 \
    && install -D -m 0555 "$api_test" /witness/aura-historia-api-tests \
    && install -D -m 0555 "$worker_test" /witness/aura-historia-worker-tests \
    && install -D -m 0555 "$cron_test" /witness/aura-historia-cron-tests

# Never published or deployed. This image contains only the three compiled test
# executables and selects one ignored secure-client witness from synthetic env.
FROM runtime AS opensearch-tls-witness
USER root
COPY --from=opensearch-tls-witness-build /witness/ /usr/local/lib/aura-opensearch-witness/
RUN printf '%s\n' \
        '#!/bin/sh' \
        'set -eu' \
        'case "${AURA_OPENSEARCH_WITNESS_RUNTIME:-}" in' \
        '  api) exec /usr/local/lib/aura-opensearch-witness/aura-historia-api-tests --ignored --exact runtime::tests::opensearch_tls::should_ping_secured_opensearch_from_witness_environment --test-threads=1 ;;' \
        '  worker) exec /usr/local/lib/aura-opensearch-witness/aura-historia-worker-tests --ignored --exact preflight::opensearch_tls_tests::should_ping_secured_opensearch_from_witness_environment --test-threads=1 ;;' \
        '  cron) exec /usr/local/lib/aura-opensearch-witness/aura-historia-cron-tests --ignored --exact wiring::search_filter_periodic_match::opensearch_tls_tests::should_ping_secured_opensearch_from_witness_environment --test-threads=1 ;;' \
        '  *) exit 64 ;;' \
        'esac' > /usr/local/bin/opensearch-tls-witness \
    && chmod 0555 /usr/local/bin/opensearch-tls-witness
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/opensearch-tls-witness"]

# Explicit dev initialization only; never shipped inside a long-running application.
FROM build AS dev-bootstrap-build
RUN cargo build --release --locked -p crawler --bin bootstrap-dev

FROM runtime AS bootstrap-dev
COPY --from=dev-bootstrap-build /app/target/release/bootstrap-dev /usr/local/bin/bootstrap-dev
ENTRYPOINT ["/usr/local/bin/bootstrap-dev"]
CMD ["--help"]

# Isolated smoke tooling only; never included in the four application images.
FROM build AS smoke-build
RUN cargo build --release --locked -p crawler --bin bootstrap-local

FROM runtime AS smoke-helper
USER root
RUN apt-get update \
    && apt-get install --yes --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=smoke-build /app/target/release/bootstrap-local /usr/local/bin/bootstrap-local
USER 10001:10001
ENTRYPOINT ["/usr/bin/python3"]
