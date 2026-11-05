FROM debian:bookworm-slim AS verifier
ARG TARGETARCH
COPY craftsman-linux-x86_64 /tmp/craftsman-linux-x86_64
COPY craftsman-linux-arm64 /tmp/craftsman-linux-arm64
COPY SHA256SUMS /tmp/SHA256SUMS
RUN cd /tmp \
 && case "$TARGETARCH" in amd64) asset=craftsman-linux-x86_64 ;; arm64) asset=craftsman-linux-arm64 ;; *) exit 1 ;; esac \
 && grep "  ${asset}$" SHA256SUMS | sha256sum --check - \
 && test "$(grep -a -c -F -e 'emSign ECC Root CA - G3' -e 'emSign Root CA - C1' -e 'emSign ECC Root CA - C3' "$asset")" = 0 \
 && install -m 0555 "$asset" /out-craftsman

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libasound2t64 \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 65532 craftsman \
 && useradd --uid 65532 --gid 65532 --no-create-home --home-dir /nonexistent craftsman
COPY --from=verifier /out-craftsman /usr/local/bin/craftsman
USER 65532:65532
RUN /usr/local/bin/craftsman --version
ENTRYPOINT ["/usr/local/bin/craftsman", "service-agent"]
