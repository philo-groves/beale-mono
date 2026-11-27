#!/usr/bin/env bash
# Native Linux gates must not discover operator credentials or provider CLIs.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_bin="$(command -v cargo)"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
test_home="$(mktemp -d /tmp/beale-runtime-tests.XXXXXXXX)"
cleanup() {
    if [[ "$test_home" == /tmp/beale-runtime-tests.* ]]; then
        rm -rf -- "$test_home"
    fi
}
trap cleanup EXIT
mkdir -p -- "$test_home/bin" "$test_home/config" "$test_home/data" "$test_home/cache"
for provider in claude codex zcode; do
    printf '#!/bin/sh\nexit 127\n' > "$test_home/bin/$provider"
    chmod 700 "$test_home/bin/$provider"
done
if [[ $# == 0 ]]; then
    set -- test -p beale-tools -p beale-storage -p beale-sessions --all-features
fi
cd -- "$repo_root"
env -i \
    HOME="$test_home" \
    XDG_CONFIG_HOME="$test_home/config" \
    XDG_DATA_HOME="$test_home/data" \
    XDG_CACHE_HOME="$test_home/cache" \
    PATH="$test_home/bin:$(dirname -- "$cargo_bin"):/usr/bin:/bin" \
    CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" \
    CARGO_TARGET_DIR="$repo_root/target/linux" \
    "$cargo_bin" "$@"
