#!/usr/bin/env bash

set -uvex -o pipefail

cd $(dirname ${BASH_SOURCE[0]})/../

which typos || cargo install typos-cli

if [[ ${OSTYPE} == "linux-gnu"* ]]; then
    which cargo-deb || cargo install cargo-deb
fi

BUILD_COMMON="--locked --profile release"

typos
cargo clippy ${BUILD_COMMON} --all-targets --all-features -- -D warnings -D clippy::pedantic -A clippy::missing_errors_doc -A clippy::module_name_repetitions
cargo clippy ${BUILD_COMMON} --tests --all-targets --all-features -- -D warnings
cargo fmt --check
cargo build ${BUILD_COMMON}
cargo test ${BUILD_COMMON}
cargo run ${BUILD_COMMON} -- readme > README.md
git diff --exit-code README.md

if [[ ${OSTYPE} == "linux-gnu"* ]]; then
    cargo deb
fi
