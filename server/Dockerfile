# Build context is the project root.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY proto proto
COPY server server
WORKDIR /src/server
# The tests must pass for the image to build.
RUN cargo test --release && cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 chat && mkdir /data && chown chat /data
COPY --from=build /src/server/target/release/server /usr/local/bin/chat-server
USER chat
VOLUME /data
EXPOSE 8080
CMD ["chat-server"]
