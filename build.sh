#!/bin/bash
set -e
cd "$(dirname "$0")"
cd client
trunk build --release
cd ../server
cargo build --release
cd ..
rm -rf dist
mkdir -p dist
cp target/release/server dist/server
cp -r ./client/dist/* dist/

echo "Build completed successfully. Files are in the 'dist' directory."