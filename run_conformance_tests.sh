#!/bin/sh
cd "$(dirname "$(readlink -f "$0")")"
python3 tools/extract_kotlin_tests.py
cargo test --jobs 1 --test conformance -- --nocapture
