//! `map:` namespace — Kap's native immutable hashmap type (`APLMap`).
//!
//! Ground truth: `~/Apps/array/array/.../builtins/map.kt` (`MapWithFunction`,
//! `MapGetFunction`, `MapRemoveKeysFunction`, `MapKeyValuesFunction`,
//! `MapSizeFunction`, `MapKeysFunction`) + `types.kt` `APLMap` (lines 915–1022).
//!
//! Kotlin's map is an `ImmutableMap2<APLValueKey, APLValue>` keyed by
//! `APLValueKey.CmpKey` — a key whose equality uses
//! `compareEqualsTotalOrdering` (value equality, type-discriminating: a Long `1`
//! and a Double `1.0` are DIFFERENT keys, but `1` equals `1`). We model the
//! content as an ordered `Vec<(K,Rc<APLValue>)>` (faithful to Kotlin's
//! `ImmutableMap2` insertion order) and compare keys with the same value-equal
//! rule the rest of the engine uses (type-discriminating numeric compare). Maps in
//! Kap are small (the stdlib uses them for help/namespace tables), so linear scan
//! is fine and avoids a fragile custom `Hash`.

use crate::{APLValue, AplRef};
use crate::number::KapNumber;
use std::cmp::Ordering;
use std::rc::Rc;

/// Map-key equality (Kotlin `APLValueKey.CmpKey` / `makeTypeQualifiedKey`).
/// Keys are compared by VALUE with TYPE DISCRIMINATION (`td=true`): a Long `1`
/// and a Double `1.0` are DIFFERENT keys, but `1` equals `1`. Faithful to
/// Kotlin's `compareEqualsTotalOrdering(td=true)`.
pub fn values_key_equal(a: &APLValue, b: &APLValue) -> bool {
    match (a, b) {
        (APLValue::Number(x), APLValue::Number(y)) => {
            match (x, y) {
                (KapNumber::Complex(ar, ai), KapNumber::Complex(br, bi)) => ar == br && ai == bi,
                (KapNumber::Complex(_, ai), _) | (_, KapNumber::Complex(_, ai)) if ai != &0.0 => false,
                _ => x.numeric_cmp(y, true).map_or(false, |o| o == Ordering::Equal),
            }
        }
        (APLValue::Char(x), APLValue::Char(y)) => x == y,
        (APLValue::Str(x), APLValue::Str(y)) => x == y,
        (APLValue::Null, APLValue::Null) => true,
        (APLValue::Symbol { name: n1, namespace: ns1 }, APLValue::Symbol { name: n2, namespace: ns2 }) => {
            n1 == n2 && ns1 == ns2
        }
        (APLValue::Array(x), APLValue::Array(y)) => {
            if x.dimensions != y.dimensions {
                return false;
            }
            let xe = x.elements();
            let ye = y.elements();
            xe.len() == ye.len() && xe.iter().zip(ye.iter()).all(|(p, q)| values_key_equal(p.as_ref(), q.as_ref()))
        }
        (APLValue::List(x), APLValue::List(y)) => {
            let xe = x.elements();
            let ye = y.elements();
            xe.len() == ye.len() && xe.iter().zip(ye.iter()).all(|(p, q)| values_key_equal(p.as_ref(), q.as_ref()))
        }
        _ => false,
    }
}

/// A Kap map. Content is stored insertion-ordered; key lookup is by value-equal
/// (type-discriminating, per Kotlin `makeTypeQualifiedKey`).
#[derive(Debug, Clone)]
pub struct KapMap {
    /// `(key, value)` pairs in insertion order.
    pub pairs: Vec<(Rc<APLValue>, Rc<APLValue>)>,
}

impl KapMap {
    pub fn new(pairs: Vec<(Rc<APLValue>, Rc<APLValue>)>) -> Self {
        KapMap { pairs }
    }

    pub fn empty() -> Self {
        KapMap { pairs: Vec::new() }
    }

    /// Number of key-value pairs.
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Look up `key` by value-equal (type-discriminating). Returns the value or
    /// `None` if absent. Mirrors Kotlin `APLMap.lookupValue`.
    pub fn lookup(&self, key: &APLValue) -> Option<Rc<APLValue>> {
        self.pairs
            .iter()
            .find(|(k, _)| values_key_equal(k, key))
            .map(|(_, v)| v.clone())
    }

    /// Insert/replace a single key (immutably): returns a new `KapMap` with the
    /// pair set. Kotlin `content.copyAndPut`.
    pub fn with_pair(&self, key: Rc<APLValue>, value: Rc<APLValue>) -> KapMap {
        let mut pairs = self.pairs.clone();
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| values_key_equal(k, &key)) {
            slot.1 = value;
        } else {
            pairs.push((key, value));
        }
        KapMap { pairs }
    }

    /// Insert/replace many pairs (immutably). Kotlin `copyAndPutMultiple`.
    pub fn with_pairs(&self, new: &[(Rc<APLValue>, Rc<APLValue>)]) -> KapMap {
        let mut map = self.clone();
        for (k, v) in new {
            map = map.with_pair(k.clone(), v.clone());
        }
        map
    }

    /// Remove the given keys (by value-equal). Kotlin `content.copyWithoutMultiple`.
    pub fn without(&self, to_remove: &[Rc<APLValue>]) -> KapMap {
        let pairs: Vec<(Rc<APLValue>, Rc<APLValue>)> = self
            .pairs
            .iter()
            .filter(|(k, _)| !to_remove.iter().any(|r| values_key_equal(k, r)))
            .cloned()
            .collect();
        KapMap { pairs }
    }

    /// Build a `2 × N` nested array of `[key value, key value, …]`. Kotlin
    /// `APLMap.aplMapToArray` returns `dimensionsOfSize(size/2, 2)` — i.e. an
    /// `N`-row, 2-column matrix. We produce a rank-2 `APLValue::Array`.
    pub fn to_array(&self) -> APLValue {
        use crate::array::{ArrayData, KapArray};
        let mut elems: Vec<Rc<APLValue>> = Vec::with_capacity(self.pairs.len() * 2);
        for (k, v) in &self.pairs {
            elems.push(k.clone());
            elems.push(v.clone());
        }
        let n = self.pairs.len();
        APLValue::Array(Rc::new(KapArray::new(vec![n, 2], ArrayData::Nested(elems))))
    }

    /// Keys as a rank-1 array (Kotlin `MapKeysFunction` → `APLArrayImpl(dimensionsOfSize(array.size), array)`).
    pub fn keys_array(&self) -> APLValue {
        use crate::array::{ArrayData, KapArray};
        let elems: Vec<Rc<APLValue>> = self.pairs.iter().map(|(k, _)| k.clone()).collect();
        APLValue::Array(Rc::new(KapArray::new(vec![elems.len()], ArrayData::Nested(elems))))
    }
}
