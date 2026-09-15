# Kap Extensions — how to add a Rust primitive in 10 minutes

> You are reading the **user guide** for the native extension system. The
> full RFC with trait rationale and diagrams is at
> `docs/native-api-design.html` (§01–§12). This file is the
> *do-it-now* manual — copy-paste friendly, no prior Kap internals needed.

---

## 1. What is an extension?

A Kap **primitive** like `⍳` `≢` `+` `map:with` is just a Rust function that
takes Kap values and returns a Kap value. An **extension crate** (`kap-ext-*`)
is a normal Rust library that gives Kap one or more new names in a
namespace, e.g. `stats:mean` `stats:median` `my:hello`.

* **One crate = one domain.** `kap-ext-stats` holds `stats:*`,
  `kap-ext-sql` would hold `sql:*`. Each crate only pulls the deps it
  needs (`rusqlite`, `csv`, …), so the base `kap` binary stays small.
* **Qualified names never collide.** Core keeps `+` `⍳` `≢`;
  you keep `my:`, `stats:`, `ext:` — the registry is a flat
  `HashMap<String, Box<dyn NativeFn>>`, uniqueness is the string
  `"stats:mean"`.
* **Linking *is* registration.** You add one line to `kap-cli/Cargo.toml`
  — `kap-ext-my = { path = "../kap-ext-my" }` — and `Engine::new()`
  finds your `inventory::submit!` statics automatically. No `dlopen`,
  no runtime `load()` call, no second list to keep in sync.

The reference crate is already in the repo: **`kap-ext-stats`**
(`stats:mean` / `median` / `stdev` / `contains`). Open it side-by-side
with this guide.

---

## 2. Prerequisites

```bash
rustc --version   # any recent stable (workspace is edition 2021)
cargo --version

# the repo — main is at a47dd77 or later, all branches at 100% (2987/0/0)
git clone https://github.com/kapitaali/rust-kap
cd rust-kap
cargo test -p kap-ext-stats   # 2 passed → your toolchain is good
```

You do **not** need the Kotlin oracle or the JVM — `stats:*` is pure
Rust. Only `jvm:` primitives need `JAVA_HOME`.

---

## 3. Quick start — your first Kap primitive

We will add `my:hello y → "hello, y"` and call it from Kap.

### 3.1 Create the crate

```bash
cargo new --lib kap-ext-my
# add it to the workspace (Cargo.toml at repo root):
# [workspace]
# members = ["kap-core","kap-stdlib","kap-cli","kap-derive","kap-ext-stats","kap-ext-my"]
```

Edit `kap-ext-my/Cargo.toml`:

```toml
[package]
name = "kap-ext-my"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "my Kap extensions — hello world"

[dependencies]
kap-core = { path = "../kap-core" }
inventory = "0.3"
# kap-derive is optional — you can use manual impls (shown here)
```

### 3.2 Write the primitive (`kap-ext-my/src/lib.rs`)

```rust
use std::rc::Rc;
use kap_core::native::{Args, NativeContext, NativeFn, NativeReg};
use kap_core::native::helpers::{bad_arg, ok};
use kap_core::{APLValue, AplError, AplRef};

#[derive(Debug, Default)]
pub struct MyHello;

impl NativeFn for MyHello {
    fn name(&self) -> &str { "my:hello" }          // Kap spelling
    fn doc(&self) -> &str { "my:hello y — greet y" }  // for )help / --list-natives
    fn call(&self, _ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
        let v = args.mono_named("my:hello")?;       // valence check → oracle-exact error
        let s = match v.as_ref() {
            APLValue::Str(s) => s.clone(),
            APLValue::Char(c) => c.to_string(),
            _ => return Err(bad_arg("my:hello", "requires a string")),
        };
        ok(APLValue::Str(format!("hello, {}", s)))
    }
}

// This line is the whole "registration". No other file touched.
inventory::submit! { NativeReg { name: "my:hello", factory: || Box::new(MyHello) } }
```

That is 18 lines of logic. `my:hello` is now a Kap name.

### 3.3 Link it into `kap`

```toml
# kap-cli/Cargo.toml
[dependencies]
kap-core = { path = "../kap-core" }
kap-ext-stats = { path = "../kap-ext-stats" }
kap-ext-my = { path = "../kap-ext-my" }   # ← add
```

```rust
// kap-cli/src/main.rs — one line so Cargo doesn't dead-code-eliminate the crate
#[allow(unused_imports)] use kap_ext_my as _;
#[allow(unused_imports)] use kap_ext_stats as _;
```

`Engine::new()` does `for reg in inventory::iter::<NativeReg> { map.insert(reg.name, (reg.factory)()) }`
— your `my:hello` appears alongside `+` `⍳` `stats:mean` automatically.

### 3.4 Build and try it

```bash
cargo build -p kap-cli

# Kap REPL (strings are Rank-1 [len]; empty "" is rank-0)
printf 'my:hello "world"\n' | ./target/debug/kap --no-standard-lib
# → "hello, world"

# check the registry (debug helper — behind env var, not spam)
KAP_DEBUG_NATIVES=1 printf 'my:hello "x"\n' | ./target/debug/kap --no-standard-lib 2>&1 | head -1
# → KAP natives: ["stats:contains", "stats:median", "stats:mean", "stats:stdev", "my:hello"]

cargo test -p kap-ext-my   # see §6
```

Set `KAP_DEBUG_NATIVES=1` once if a name does not appear — it
means the crate was not linked (missing `use kap_ext_my as _;`).

---

## 4. Anatomy of a primitive

```
                  ┌─────────────┐  name = "my:hello" ──┐
     Kap source   │  my:hello   │──────────────────────►│
     "my:hello    │   "world"   │   parser gate         │  is_known_fn
     "world""     │             │   is_known_fn → true  │  (parser.rs, derived
                  └──────┬──────┘   so it parses as     │   from inventory)
                         │          L f R                │
                         ▼                               │
                  ┌─────────────┐   eval gate           │  is_primitive_name
                  │ eval_apply  │──► is_primitive_name  │  (evaluator.rs,
                  │  natives    │   → true               │   derived)
                  │  .get(name) │                        │
                  └──────┬──────┘                        │
                         │  NativeContext{engine, env}   │
                         ▼  Args::Monad(v) or Dyad(a,b) │
                  ┌─────────────┐                        │
                  │ your impl   │  call(ctx, args)       │
                  │ MyHello     │────────────────────────┘
                  └──────┬──────┘   Result<AplRef<APLValue>, AplError>
                         │
                         ▼   Ok → Kap value; Err → "my:hello: …" in REPL
```

* **Parser gate** (`parser.rs::is_known_fn`) — decides if `my:hello`
  followed by an argument builds an `Apply` node (preserving the left
  operand for dyadics) or stays a plain `Symbol` stranding.
  Derived from the registry — no hand list.

* **Eval gate** (`evaluator.rs::is_primitive_name` + `eval_apply`) —
  looks up `self.natives.get(name)` before the legacy giant `match`.
  If found, calls `native.call(&ctx, args)` and returns; otherwise
  falls through to the old `match` (so `+` `⍳` still work during
  migration).

---

## 5. The `NativeFn` trait — line by line

```rust
pub trait NativeFn: std::fmt::Debug + 'static {
    fn name(&self) -> &str;                 // required — "stats:mean"
    fn arity(&self) -> Arity { Arity::Monadic }  // Monadic / Dyadic / Ambivalent
    fn rank(&self) -> RankSpec { RankSpec::Any } // Any / Scalar / Vector / Fixed(n)
    fn secure_ok(&self) -> bool { true }    // false → omitted in secureMode
    fn doc(&self) -> &str { "" }            // for )help
    fn call(&self, ctx: &NativeContext<'_>, args: Args) -> Result<AplRef<APLValue>, AplError>;
}
```

* `Arity` — most new fns are `Monadic` (the default) or `Dyadic`.
  `Ambivalent` is for `+` `×` style. `args.mono_named(name)` /
  `args.dyad_named(name)` emit `"<name>: requires one/two arguments"`
  exactly like Kotlin.

* `RankSpec` — `Any` (you handle rank, the default, like `⍴` `⍉`);
  `Scalar` (framework does rank-0 broadcast, `Str` as `[len]`);
  `Vector` / `Fixed(2)` for rank-checked table ops.

* `NativeContext { engine: &Engine, env: &AplRef<Environment> }` —
  only for the few fns that need it (`isLocallyBound` checks
  `env.lexical_contains`, `use` needs `engine.lib_paths`). Most
  primitives ignore `ctx`.

* `secure_ok() -> false` — equivalent to listing the name in the old
  `is_secure_gated` match. The engine rejects it in secure mode as
  `Variable not assigned: stats:foo` (same as `io:read` in
  `SecureTest.kt`).

All other helpers live in `kap_core::native::helpers` and
`kap_core::native::prelude`.

---

## 6. Working with values

### 6.1 The value model (`APLValue`)

```rust
pub enum APLValue {
    Number(KapNumber),          // Long / Double / BigInt / Rational / Complex
    Char(char),                 // @a
    Str(String),                // "hello" — rank-1 [len], empty "" is rank-0
    Array(Rc<KapArray>),        // (1 2 3) etc.
    List(Rc<KapArray>),         // (1;2;3) — ; separates, typeof → kap:list
    Null,                       // ⍬  (empty array, rank-1 [0])
    Nil,                        // null (rank-0, distinct from ⍬)
    // … Jvm, Map, Stream, … — see `kap-core/src/lib.rs:58`
}
pub type AplRef<T> = Rc<T>;   // single-threaded, cheap clone
```

* `v.elements() -> Vec<AplRef<APLValue>>` — flat row-major elements
  (hoist once; it rebuilds the Vec each call).
* `v.dimensions() -> Vec<usize>` / `v.rank() -> usize` /
  `v.value_at(i) -> APLValue` / `v.is_null()` / `v.is_nil()`.
* `KapNumber` — `Long(i64)` / `Double(f64)` / `BigInt` / `Rational` /
  `Complex(f64,f64)`. `n.add(&m)` `n.sub(&m)` `n.mul(&m)` `n.div(&m)`
  preserve exactness (Long → Rational on `10 ÷ 3`). `n.as_double() -> f64`
  for float paths. `KapNumber::number_ordering(a,b)` is the total order
  Kap sorts by.
* Construction:
  `Rc::new(APLValue::Number(KapNumber::Long(42)))`,
  `KapArray::new(vec![3], ArrayData::Nested(elems))`.

### 6.2 Helpers

```rust
use kap_core::native::helpers::{ok, ok_long, ok_bool, bad_arg, bad_type};
ok(APLValue::Str("hi".into()))          // Ok(Rc::new(…))
ok_long(1)                               // 1 as Long
ok_bool(true)                            // 1 / 0 as Long
bad_arg("my:hello", "requires a string") // → AplError::Runtime("my:hello: …")
```

Error text is the contract — the conformance harness checks
`kind:"fails"` rows, and users see it. Always prefix with
`"<name>: "` via `bad_arg(name, msg)`.

### 6.3 Rank & scalar extension

Most arithmetic wants *scalar extension*: rank-0 `5` plus rank-1
`1 2 3` → `6 7 8`, and `Str` `"abc"` as rank-1 `[3]`
(`"abc" + 1 → "bcd"`). Two options:

* **Manual** (`RankSpec::Any`, default) — you read `v.rank()`,
  `v.elements()`, and handle broadcast yourself. See `tally.rs`.

* **Framework** (`RankSpec::Scalar` + `scalar::apply_scalar`) — the
  trait `ScalarImpl { monad(a: &APLValue), dyad(a,b) }` is called
  per element with rank-0 broadcast already done. `Str` is treated as
  `[len]` automatically:

```rust
struct MyAdd; struct MyAddImpl;
impl NativeFn for MyAdd {
    fn name(&self) -> &str { "my:add" }
    fn arity(&self) -> Arity { Arity::Ambivalent }
    fn rank(&self) -> RankSpec { RankSpec::Scalar }
    fn call(&self, _: &NativeContext, args: Args) -> Result<…, AplError> {
        scalar::apply_scalar(&MyAddImpl, "my:add", args)
    }
}
impl scalar::ScalarImpl for MyAddImpl {
    fn monad(&self, a: &APLValue) -> Result<APLValue, AplError> { Ok(a.clone()) }
    fn dyad(&self, a: &APLValue, b: &APLValue) -> Result<APLValue, AplError> {
        match (a, b) { (APLValue::Number(x), APLValue::Number(y)) => Ok(APLValue::Number(x.add(y))), _ => Err(bad_arg("my:add","type mismatch")) }
    }
}
```

---

## 7. Sweet sugar — `#[kap_fn]` (optional)

For the common case, `kap-derive` offers a proc-macro. Today it is a
pass-through (so `kap-ext-stats` uses manual `inventory::submit!`);
the next cut will make it generate the struct + impl + submit:

```rust
use kap_core::native::prelude::*;
#[kap_fn(name = "my:hello", doc = "my:hello y — greet y")]
fn my_hello(_ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
    let v = args.mono_named("my:hello")?;
    // … same body
    Ok(Rc::new(APLValue::Str(format!("hello, {}", s))))
}
// expands (next cut) to:
// struct MyHello; impl NativeFn for MyHello { fn name…; fn call… { my_hello(ctx, args) } }
// inventory::submit! { NativeReg { name: "my:hello", factory: || Box::new(MyHello) } }
```

Keep using manual `struct + impl + inventory::submit!` for now — call
sites will not change.

---

## 8. Testing

### 8.1 Unit — call the impl directly (fastest)

```rust
#[cfg(test)] mod tests {
    use super::*;
    use kap_core::{Engine, Environment};
    #[test] fn hello_str() {
        let f = MyHello;
        let ctx = NativeContext { engine: &Engine::new(), env: &Environment::new_root() };
        let v = Rc::new(APLValue::Str("world".into()));
        let got = f.call(&ctx, Args::Monad(v)).unwrap();
        assert_eq!(got.format_value(), "hello, world");
    }
    #[test] fn hello_valence() {
        let f = MyHello;
        let ctx = NativeContext { engine: &Engine::new(), env: &Environment::new_root() };
        let a = Rc::new(APLValue::Number(KapNumber::Long(1)));
        let b = Rc::new(APLValue::Number(KapNumber::Long(2)));
        let err = f.call(&ctx, Args::Dyad(a, b)).unwrap_err();
        assert!(err.to_string().contains("requires one argument"));
    }
}
```
```bash
cargo test -p kap-ext-my
```

### 8.2 Kap-level — the real gate (still the release criterion)

```bash
# the new name is now a real Kap primitive — use Engine or the REPL
printf 'my:hello "Ada"\n' | cargo run -p kap-cli -- --no-standard-lib
# → "hello, Ada"

# the full 2987-row oracle must stay green after any native change
export JAVA_HOME=/home/theb/Apps/jdk-current
cargo test --release -p kap-core --test conformance run_kotlin_conformance -- --nocapture
# → 2987 ok / 0 mismatch / 0 unsupported = 100.0%

# whole workspace
cargo test --release -p kap-core           # 114 + lib tests
cargo test -p kap-ext-stats                # 2 ext tests
```

Add a `curated_kap_parity` row if the oracle text matters
(`kap-core/tests/conformance.rs` curated list).

---

## 9. Linking, namespaces & distribution

* **Name rule** — use a qualified name: `stats:mean` `my:hello`
  `acme:fetch`. Bare names (`mean`) risk colliding with future core
  glyphs. Reserve `kap:` for core; `stats:` `my:` `ext:` for you.
  The registry is flat, uniqueness is the string.

* **One crate per domain** — `kap-ext-sql` (pulls `rusqlite`),
  `kap-ext-arrow` (pulls `arrow-rs`), `kap-ext-csv` — each only pays
  for its deps. The base `kap` binary stays small without `features`.

* **Linking** — add one line:
  `kap-ext-my = { path = "../kap-ext-my" }` to `kap-cli/Cargo.toml`
  (workspace `Cargo.toml` members too) and keep
  `use kap_ext_my as _;` in `kap-cli/src/main.rs` so the linker
  keeps the `inventory` statics. `KAP_DEBUG_NATIVES=1` lists what's
  linked; empty means the crate was dead-code-eliminated.

* **Alternative — feature-gated core** — if you prefer one repo/no
  extra crates, keep everything in `kap-core/src/native/` behind
  `features = ["ext-my"]` and guard the `inventory::submit!` with
  `#[cfg(feature="ext-my")]`. Same trait, same registry.

* **`secureMode`** — a primitive that touches the filesystem or network
  must declare `fn secure_ok(&self) -> bool { false }` (or
  `#[kap_fn(secure=false)]` later). The engine then rejects it in
  secure mode as `Variable not assigned: my:fetch` — same as `io:read`.

* **Help** — `fn doc(&self) -> &str` feeds `)help my:hello` / future
  `kap --list-natives`. Write one sentence.

---

## 10. Reference — `kap-ext-stats` walkthrough

Open `kap-ext-stats/src/lib.rs` — 140 lines, 4 primitives:

| Kap | Rust `struct` | `call` sketch |
|-----|---------------|---------------|
| `stats:mean y` | `StatsMean` | `sum( KapNumber::add ) / len.to_rational` — preserves BigInt/Rational exactness |
| `stats:median y` | `StatsMedian` | `sort_by(KapNumber::number_ordering)` → middle or `(a+b)/2` |
| `stats:stdev y` | `StatsStdev` | `as_double()` → f64 mean/var/sqrt → `KapNumber::Double` |
| `x stats:contains y` | `StatsContains` | `Dyadic` — `hay.elements().any(|e| e.format_value()==needle.format_value())` |

Each is:
```rust
#[derive(Debug, Default)] pub struct StatsMean;
impl NativeFn for StatsMean { fn name…; fn call… }
inventory::submit! { NativeReg { name: "stats:mean", factory: || Box::new(StatsMean) } }
```
Copy one block, rename `StatsMean` → `MyFoo`, change `name()` and the
element logic — you have a new primitive.

---

## 11. Troubleshooting

* **`Variable not assigned: my:hello`** or **`undefined symbol: hello`**
  — parser did not see the name as a function. Check:
  `is_known_fn` / `is_primitive_name` now derive from the registry — so
  the cause is almost always *linking*: `kap-cli/Cargo.toml` missing
  the `kap-ext-my` dep, or `kap-cli/src/main.rs` missing
  `use kap_ext_my as _;`. Verify with `KAP_DEBUG_NATIVES=1`.

* **`KAP natives: []`** — empty registry → same linking cause, or you
  built `kap-core` alone (`cargo test -p kap-core` does not link
  `kap-ext-*`; only `kap-cli` does).

* **`requires one/two arguments`** with wrong name — you called
  `args.mono()` not `args.mono_named("my:hello")`. Use the `_named`
  helpers so the error prefix is oracle-exact.

* **Length mismatch on dyadics** — both args rank ≥1 with different
  `elements().len()`. Use `RankSpec::Scalar` + `scalar::apply_scalar`
  if you want scalar extension.

* **Secure mode hides your fn** — `fn secure_ok(&self) -> bool { false }`
  deliberately omits it when `Engine::set_secure_mode(true)` (harness
  does this for `SecureTest.kt`). Most `stats:` fns should return `true`.

* **Str vs Char confusion** — `Str("")` is rank-0 (like a scalar), not
  `[0]`. `Str("ab")` is rank-1 `[2]`. The scalar helper handles this;
  manual code should use `v.rank()` / `v.dimensions()`.

---

## 12. Where to go next

* **Read the RFC** — `docs/native-api-design.html` §§01–§12 for the
  trait rationale, registry diagram, `ScalarImpl` deep dive, rank
  table, and migration plan (`evaluator.rs` 16 k → 6 k LOC after bulk
  move).
* **Migrate a core primitive** — pick one `kap-core/src/evaluator.rs`
  arm (`≢`, `⍳`, `⍴`, …), `mv` it to `kap-core/src/native/my_fn.rs`
  as a `NativeFn` impl, delete the old arm, re-run `2987/0/0`. Each
  PR is ~1 file, no flag day.
* **Operators next** — `NativeOp { apply(ctx, left: Option<Box<Instr>>, right: Box<Instr>) }`
  for adverbs (`¨` `⍤` `⍣` `⍢` `˝`) will use the same registry. Not in
  this cut — functions first, operators second.

*Questions?* Open `kap-core/src/native/mod.rs` — 280 lines, fully
commented — and `kap-ext-stats/src/lib.rs` as the living example.
