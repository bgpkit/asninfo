# select build image
FROM rust:1.85.1 AS build

# create a new empty shell project
RUN USER=root cargo new --bin my_project
WORKDIR /my_project

# copy your source tree
COPY ./src ./src
COPY ./Cargo.toml .

# build for release
RUN cargo build --release --all-features

# our final base
FROM debian:bookworm-slim

# copy the build artifact from the build stage
COPY --from=build /my_project/target/release/asninfo /usr/local/bin/asninfo

WORKDIR /asninfo

ENTRYPOINT ["/usr/local/bin/asninfo"]
