#!/bin/sh
# Builds dfc for Linux and Windows and puts the release archives in dist/.
set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
# Keep the build machine's paths (home, crate registry) out of the binaries' panic messages.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=."
cargo test --release
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu

rm -rf dist
mkdir -p dist/stage

lin=dfc-$version-linux-x86_64
mkdir dist/stage/$lin
cp target/release/dfc README.md dist/stage/$lin/
tar -C dist/stage -czf dist/$lin.tar.gz $lin

win=dfc-$version-windows-x86_64
mkdir dist/stage/$win
cp target/x86_64-pc-windows-gnu/release/dfc.exe dist/stage/$win/
sed 's/$/\r/' README.md > dist/stage/$win/README.txt
(cd dist/stage && zip -qr ../$win.zip $win)

rm -rf dist/stage
(cd dist && sha256sum *.tar.gz *.zip > SHA256SUMS)
ls -l dist
