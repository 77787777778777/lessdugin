# SPDX-License-Identifier: AGPL-3.0-or-later
# Build from the repository root. `.github/workflows/indexer.yml` publishes M6 builds to GHCR.
FROM docker.io/library/rust:1.97-bookworm AS build
ARG SOURCE_URL=https://github.com/lessdugin/lessdugin
ENV LESSDUGIN_INDEXER_SOURCE_URL=${SOURCE_URL}
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p lessdugin-indexer

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/lessdugin-indexer /usr/local/bin/lessdugin-indexer
VOLUME ["/data"]
EXPOSE 2584
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/lessdugin-indexer", "--listen", "0.0.0.0:2584", "--data-dir", "/data"]
