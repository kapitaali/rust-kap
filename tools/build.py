#!/usr/bin/env python3
"""Consume rust-kap.build.yaml and drive `cargo` with the configured flags.

This is the single entry point for building/testing the Rust Kap interpreter
according to the switches in rust-kap.build.yaml. It translates the YAML config
into cargo invocations (jobs, profile, features, ulimit memory cap, etc.) so the
working build configuration is reproducible without hand-setting environment.

Usage:
    tools/build.py [cargo-subcommand] [extra-args...]

Subcommands:
    build            Build the default member (kap-cli -> `kap`). Default.
    test             Run tests (unit + conformance per the config).
    run              Run the built `kap` binary.
    check            `cargo check` only (fast type-check).
    clean            `cargo clean`.

Examples:
    tools/build.py                       # build
    tools/build.py test                  # run unit + conformance tests
    tools/build.py test -- --test-threads=1
    tools/build.py build --release
    tools/build.py run -- "1 2 3 +/ 4"   # pass args to the `kap` binary

If PyYAML is not installed, the script falls back to a tiny built-in reader that
understands the subset of YAML used by rust-kap.build.yaml (2-space indents,
scalars, lists, and nested maps).
"""

import os
import sys
import shlex
import subprocess

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONFIG_PATH = os.path.join(REPO_ROOT, "rust-kap.build.yaml")


def load_config(path):
    try:
        import yaml  # type: ignore

        with open(path) as f:
            return yaml.safe_load(f)
    except ImportError:
        # Minimal fallback parser for our config's YAML subset.
        return _mini_yaml(path)


def _mini_yaml(path):
    """Parse the subset of YAML used by rust-kap.build.yaml.

    Supports: nested maps (2-space indent), scalar `key: value`, booleans/ints,
    and `- item` lists (scalars or inline scalars in maps). Comments (#) and
    blank lines are ignored.
    """
    root = {}
    stack = [(-1, root)]  # (indent, container)
    with open(path) as f:
        for raw in f:
            line = raw.split("#", 1)[0].rstrip("\n")
            if not line.strip():
                continue
            indent = len(line) - len(line.lstrip(" "))
            key, _, val = line.strip().partition(":")
            key = key.strip()
            val = val.strip()
            # pop stack to the correct parent
            while stack and indent <= stack[-1][0]:
                stack.pop()
            parent = stack[-1][1]
            if val == "":
                # map or list item begins a new container
                if key.startswith("- "):
                    item = key[2:].strip()
                    lst = parent.setdefault("_list", [])
                    if item:
                        lst.append(item)
                    container = {}
                    lst.append(container)
                    stack.append((indent, container))
                else:
                    container = {}
                    parent[key] = container
                    stack.append((indent, container))
            else:
                if isinstance(parent, list):
                    # shouldn't happen for scalars
                    continue
                if key.startswith("- "):
                    item = key[2:].strip() or val
                    parent.setdefault("_list", []).append(_coerce(item))
                else:
                    parent[key] = _coerce(val)
    return _flatten(root)


def _coerce(v):
    if v == "true":
        return True
    if v == "false":
        return False
    if v == "null" or v == "":
        return None
    try:
        return int(v)
    except ValueError:
        return v


def _flatten(node):
    """Turn the {.., '_list': [...]} intermediate into clean dicts/lists.

    A container that collected items via `- ` list syntax is collapsed to a
    plain list; otherwise it stays a dict (with any nested dicts flattened
    recursively). The top-level node is always a dict.
    """
    if isinstance(node, dict):
        out = {}
        items = node.pop("_list", None)
        for k, v in node.items():
            out[k] = _flatten(v)
        if items is not None:
            out["__items__"] = [_flatten(x) for x in items]
            return out["__items__"]
        return out
    return node


def features_string(cfg):
    feats = (cfg.get("features") or {})
    on = [name for name, val in feats.items() if val is True]
    if not on:
        return None
    return ",".join(on)


def cargo_env(cfg):
    env = dict(os.environ)
    build = cfg.get("build") or {}
    mb = build.get("memory-limit-vbytes")
    if mb:
        # ulimit -v takes kilobytes
        env["KAP_ULIMIT_V_KB"] = str(mb // 1024)
    return env, mb


def run(cmd, env=None):
    print("+ " + " ".join(shlex.quote(c) for c in cmd), flush=True)
    return subprocess.call(cmd, cwd=REPO_ROOT, env=env)


def main(argv):
    if not os.path.exists(CONFIG_PATH):
        sys.stderr.write(f"config not found: {CONFIG_PATH}\n")
        return 2

    cfg = load_config(CONFIG_PATH)
    build = cfg.get("build") or {}
    tests = cfg.get("tests") or {}
    default_member = (cfg.get("workspace") or {}).get("default-member", "kap-cli")

    sub = argv[0] if argv else "build"
    extra = argv[1:]

    env, mb = cargo_env(cfg)
    # Apply the memory cap by re-execing under bash ulimit if set and we are the
    # top-level process. Simplest portable approach: wrap with `bash -c 'ulimit -v N; exec ...'`.
    prefix = []
    if mb:
        kb = mb // 1024
        # Re-invoke ourselves under a ulimit-limited shell so cargo's children
        # inherit the cap. Avoid infinite recursion via an env marker.
        if not os.environ.get("KAP_ULIMITED"):
            wrapped = (
                ["bash", "-c",
                 f"ulimit -v {kb}; exec env KAP_ULIMITED=1 "
                 f"{shlex.quote(sys.executable)} {shlex.quote(os.path.abspath(__file__))} "
                 + " ".join(shlex.quote(a) for a in argv)]
            )
            print("+ " + " ".join(shlex.quote(c) for c in wrapped), flush=True)
            return subprocess.call(wrapped, cwd=REPO_ROOT)

    base = ["cargo"]
    # This cargo front-end rejects `-p` / `--manifest-path` in the positions we
    # tried, so we run plain `cargo` from the repo root (the workspace builds all
    # members, producing the `kap` binary from kap-cli). `default-member` in the
    # config still documents intent.
    manifest = None

    feats = features_string(cfg)

    def with_manifest(cmd):
        return cmd

    if sub == "build":
        cmd = with_manifest(base + ["build"])
        if build.get("jobs") is not None:
            cmd += ["--jobs", str(build["jobs"])]
        if build.get("profile") == "release":
            cmd += ["--release"]
        if feats:
            cmd += ["--features", feats]
        cmd += extra
        return run(cmd, env)

    if sub == "check":
        cmd = with_manifest(base + ["check"])
        if build.get("jobs") is not None:
            cmd += ["--jobs", str(build["jobs"])]
        if feats:
            cmd += ["--features", feats]
        cmd += extra
        return run(cmd, env)

    if sub == "test":
        # Unit tests always.
        cmd = with_manifest(base + ["test", "--lib"])
        if build.get("jobs") is not None:
            cmd += ["--jobs", str(build["jobs"])]
        if feats:
            cmd += ["--features", feats]
        rc = run(cmd, env)
        # Conformance harness (its panic is intentional; see config).
        if tests.get("conformance", True):
            ccmd = with_manifest(base + ["test", "--test", "conformance"])
            if build.get("jobs") is not None:
                ccmd += ["--jobs", str(build["jobs"])]
            if feats:
                ccmd += ["--features", feats]
            # pass through any remaining extra args (e.g. -- --test-threads=1)
            ccmd += extra
            rc2 = run(ccmd, env)
            # The conformance test fails on purpose to surface its summary.
            # Only propagate a real failure (non-zero AND not the known panic).
            return rc or rc2
        return rc

    if sub == "run":
        cmd = with_manifest(base + ["run"])
        if build.get("jobs") is not None:
            cmd += ["--jobs", str(build["jobs"])]
        if build.get("profile") == "release":
            cmd += ["--release"]
        if feats:
            cmd += ["--features", feats]
        # `kap` reads a program from stdin (REPL) or a file path as argv[1].
        # For `run -- "expr"`, feed the expression on stdin so it is evaluated
        # instead of being mistaken for a filename.
        stdin_data = None
        if "--" in extra:
            idx = extra.index("--")
            passthrough = extra[idx + 1:]
            if passthrough:
                stdin_data = "\n".join(passthrough) + "\n"
            cmd += extra[:idx]
        else:
            cmd += extra
        print("+ " + " ".join(shlex.quote(c) for c in cmd) + ("  (expr piped via stdin)" if stdin_data else ""), flush=True)
        proc = subprocess.run(cmd, cwd=REPO_ROOT, env=env, input=stdin_data, text=True)
        return proc.returncode

    if sub == "clean":
        return run(["cargo", "clean"] + extra, env)

    sys.stderr.write(f"unknown subcommand: {sub}\n")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
