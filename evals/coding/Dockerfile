FROM rust:1.99.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release --no-default-features -p craftsman-rs --bin craftsman
FROM rust:1.99.1-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends python3 nodejs npm && rm -rf /var/lib/apt/lists/*
RUN npm install --global @openai/codex@0.167.0 @anthropic-ai/claude-code@2.1.322
COPY --from=build /src/target/release/craftsman /usr/local/bin/craftsman
WORKDIR /workspace
ENTRYPOINT []
CMD ["craftsman", "acp-serve"]
