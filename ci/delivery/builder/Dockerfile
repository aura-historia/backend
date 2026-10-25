# Publish once with publish-builder.sh; releases consume the resulting ECR digest.
FROM rust:1.98.0-bookworm@sha256:4e4a7e7939c17991ab35f2b8c2e67593980f771d28f6b1254b1850f860fd0c7f
RUN apt-get update && apt-get install -y --no-install-recommends python3-venv time && \
    rm -rf /var/lib/apt/lists/* && \
    rustup target add x86_64-unknown-linux-musl && \
    cargo install cargo-lambda --version 1.9.0 --locked && \
    rm -rf /usr/local/cargo/registry /usr/local/cargo/git
RUN python3 -m venv /opt/zig && \
    echo 'ziglang==0.14.1 --hash=sha256:6eb9d4d759b292c83810dbee2e9e8e3fbfbf01d864e6e9811bae711fd74e1c2f' > /tmp/zig-requirements && \
    /opt/zig/bin/pip install --no-cache-dir --require-hashes -r /tmp/zig-requirements && \
    ln -s /opt/zig/lib/python3.11/site-packages/ziglang/zig /opt/zig/bin/zig && \
    rm /tmp/zig-requirements
RUN curl --fail --location --silent --show-error https://nodejs.org/dist/v26.0.0/node-v26.0.0-linux-x64.tar.xz -o /tmp/node.tar.xz && \
    echo '345d558514c62622b5c7d1f7b5f2a19c31ab1405d217df49f010c5ea8decc0f4  /tmp/node.tar.xz' | sha256sum --check - && \
    mkdir /opt/node && tar -xf /tmp/node.tar.xz --strip-components=1 -C /opt/node && rm /tmp/node.tar.xz
RUN curl --fail --location --silent --show-error https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz -o /tmp/sccache.tar.gz && \
    echo 'aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd  /tmp/sccache.tar.gz' | sha256sum --check - && \
    tar -xf /tmp/sccache.tar.gz -C /tmp && \
    install /tmp/sccache-v0.19.0-x86_64-unknown-linux-musl/sccache /usr/local/bin/sccache && \
    rm -rf /tmp/sccache*
ENV PATH="/opt/node/bin:/opt/zig/bin:${PATH}"
RUN rustc --version && cargo lambda --version && zig version && node --version && sccache --version
