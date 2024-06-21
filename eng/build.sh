#!/usr/bin/env bash

set -uvex -o pipefail

BUILD_TARGET=${1:-$(rustc --version --verbose | grep ^host: | cut -d ' ' -f 2)}

cd $(dirname ${BASH_SOURCE[0]})/../

which typos || cargo install typos-cli

if [[ ${BUILD_TARGET} == "x86_64-unknown-linux-gnu" ]]; then
    which cargo-deb || cargo install cargo-deb
fi

BUILD_COMMON="--locked --profile release --target ${BUILD_TARGET}"

typos
cargo clippy ${BUILD_COMMON} --all-targets --all-features -- -D warnings -D clippy::pedantic -A clippy::missing_errors_doc -A clippy::module_name_repetitions
cargo clippy ${BUILD_COMMON} --tests --all-targets --all-features -- -D warnings
cargo fmt --check
cargo build ${BUILD_COMMON}
cargo test ${BUILD_COMMON}
cargo run ${BUILD_COMMON} -- readme > README.md
git diff --exit-code README.md

if [[ ${BUILD_TARGET} == "x86_64-unknown-linux-gnu" ]]; then
    cargo deb --target ${BUILD_TARGET}
fi
