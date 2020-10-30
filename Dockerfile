# We need to use the Rust build image, because we need the Rust compile and Cargo tooling
FROM rust as build

WORKDIR /app

# Build
COPY . .
RUN cargo build --release

# Create a new stage with a minimal image because we already have a binary built
# This could be scratch, but Jenkins pipelines require `cat` in the image
FROM rust
SHELL ["/bin/bash", "-c"]

# Copies the binary from the "build" stage
COPY --from=build /app/target/release/ecs-manage /bin/

# Smoke test
RUN test "$(/bin/ecs-manage --version)" == 'ecs-manage 0.1.1'

# Configures the startup!
ENTRYPOINT ["/bin/ecs-manage"]
