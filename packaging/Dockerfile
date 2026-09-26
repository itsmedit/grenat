# The official image: grenat, setter, and a C linker for `grenat build`.
#
#   docker run --rm -v "$PWD":/app ghcr.io/itsmedit/grenat test
#   docker run --rm -v "$PWD":/app -p 3000:3000 ghcr.io/itsmedit/grenat serve --listen 0.0.0.0:3000
FROM debian:bookworm-slim

ARG VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
# installed as anyone would: the release's archive, its checksum checked
COPY install.sh /tmp/install.sh
RUN sh /tmp/install.sh --version "$VERSION" --prefix /usr/local --no-modify-path && rm /tmp/install.sh

WORKDIR /app
ENTRYPOINT ["grenat"]
CMD ["--help"]
