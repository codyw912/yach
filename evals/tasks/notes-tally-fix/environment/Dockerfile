# Task container for the yacht/harbor path (portfolio track 3).
# Untested until the musl packaging item lands — the local gate mounts
# a fixture/ copy into the yach-runtime image instead. Build-context
# and workdir conventions to be confirmed against Harbor then.
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates jq \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY fixture/ /app/
