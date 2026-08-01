#!/usr/bin/env bash
set -euo pipefail

echo "Building Wick Fair Market contract..."
stellar contract build --package wick-fair-market

echo "Optimising WASM..."
stellar contract optimize \
  --wasm target/wasm32v1-none/release/wick_fair_market.wasm

echo "Build complete:"
ls -lh target/wasm32v1-none/release/wick_fair_market*.wasm
