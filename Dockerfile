# We need to use the Rust build image, because we need the Rust compile and Cargo tooling
FROM clux/muslrust:stable as build

# Install cmake as it is not included in muslrust, but is needed by libssh2-sys
RUN apt-get update && apt-get install -y \
  cmake \
  --no-install-recommends && \
  rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Build
COPY . .
RUN cargo install --target x86_64-unknown-linux-musl --path .

FROM alpine:latest as certs
RUN apk --no-cache add ca-certificates

# Create a new stage with a minimal image because we already have a binary built
# This could be scratch, but Jenkins pipelines require `cat` in the image
FROM alpine:latest

# Copies standard SSL certs from the "build" stage
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
COPY --from=certs /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# Copies the binary from the "build" stage
COPY --from=build /root/.cargo/bin/ecs-manage /bin/

# Smoke test
RUN test "$(/bin/ecs-manage --version)" == 'ecs-manage 0.1.1'

# Configures the startup!
ENTRYPOINT ["/bin/ecs-manage"]
