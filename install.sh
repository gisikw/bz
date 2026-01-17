#!/usr/bin/env bash
set -e

MODE="${1:-release}"

if [ "$MODE" = "dev" ]; then
    cargo build
    mkdir -p bin
    cp target/debug/bz bin/
    cp target/debug/bzd bin/
    echo "Dev build installed to ./bin/"
else
    cargo build --release
    mkdir -p bin
    cp target/release/bz bin/
    cp target/release/bzd bin/
    echo "Release build installed to ./bin/"
fi

echo "Symlink with:"
echo "  ln -sf $PWD/bin/bz ~/.local/bin/"
echo "  ln -sf $PWD/bin/bzd ~/.local/bin/"
