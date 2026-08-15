#!/bin/sh
python3 tools/extract_kotlin_tests.py
cargo test --jobs 1 --test conformance -- --nocapture
