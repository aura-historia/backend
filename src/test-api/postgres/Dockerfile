FROM postgres:16-bookworm

LABEL org.opencontainers.image.source="https://github.com/aura-historia/backend" \
      org.opencontainers.image.description="PostgreSQL 16 integration-test image with pg_ttl_index"

ARG PG_TTL_INDEX_VERSION=3.0.0
ARG PG_TTL_INDEX_SHA256=cd27090bf5496b7563ed0926cebce5424864db7f6c149e1da4b75042439e2d82

RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        build-essential \
        ca-certificates \
        curl \
        postgresql-server-dev-16 \
        unzip \
    && curl --fail --location --retry 3 \
        --output /tmp/pg_ttl_index.zip \
        "https://github.com/ibrahimkarimeddin/postgres-extensions-pg_ttl/releases/download/v${PG_TTL_INDEX_VERSION}/pg_ttl_index-v${PG_TTL_INDEX_VERSION}.zip" \
    && echo "${PG_TTL_INDEX_SHA256}  /tmp/pg_ttl_index.zip" | sha256sum --check \
    && mkdir /tmp/pg_ttl_index \
    && unzip -q /tmp/pg_ttl_index.zip -d /tmp/pg_ttl_index \
    && extension_dir="$(dirname "$(find /tmp/pg_ttl_index -name Makefile -print -quit)")" \
    && test -n "${extension_dir}" \
    && make -C "${extension_dir}" install \
    && rm -rf /tmp/pg_ttl_index /tmp/pg_ttl_index.zip \
    && apt-get purge --auto-remove --yes \
        build-essential \
        curl \
        postgresql-server-dev-16 \
        unzip \
    && rm -rf /var/lib/apt/lists/*
