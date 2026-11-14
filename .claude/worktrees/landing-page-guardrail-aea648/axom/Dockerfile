# syntax=docker/dockerfile:1
# Combox core and signer. Build from the repository root (BuildKit):
#   docker build -f axom/Dockerfile -t axom .
# The same image runs `axom` (API + HR loops) or `axom-signer`.
FROM rust:1.99.0-slim-bookworm AS build
WORKDIR /src
# Only what the compiler reads: migrations are embedded at compile time.
# Configs are copied into the runtime stage only, so editing one doesn't
# rebuild the binaries.
COPY supabase supabase
COPY axom axom
# Cargo's downloads and compiled dependencies are kept between builds (cache
# mounts), so changing Combox's code recompiles only Combox, not every crate.
# The target dir is a cache mount, so the binaries are copied out of it.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/axom/target,sharing=locked \
    cd axom && cargo build --release -p axom-bin -p axom-signer \
 && mkdir -p /out && cp target/release/axom target/release/axom-signer /out/

FROM debian:bookworm-20261020-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 axom
COPY --from=build /out/axom /out/axom-signer /usr/local/bin/
COPY config /app/config
WORKDIR /app
ENV AXOM_POLICY_PATH=/app/config/policy.toml \
    AXOM_PLANNER_CONFIG_PATH=/app/config/planner.toml \
    AXOM_PERFORMANCE_CONFIG_PATH=/app/config/performance.toml \
    AXOM_MASTER_PLAN_PATH=/app/config/master_plan.json \
    AXOM_API_BIND=0.0.0.0:8080 \
    AXOM_SIGNER_BIND=0.0.0.0:8090
USER axom
EXPOSE 8080 8090
CMD ["axom"]
