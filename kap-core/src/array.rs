//! Array value model for Kap (Phase 1).
//!
//! Mirrors Kap's array storage. Per strategy §4.1, array contents are stored in
//! specialised, immutable buffers (copy-on-write only where profiling demands). Each
//! variant holds one element type to avoid boxing scalars.
//!
//! Phase 1 covers the numeric + char + nested variants. String is modelled as
//! `APLValue::Str` (a 1-D char vector) at the value level; here we keep a generic
//! nested variant for mixed/boxed arrays.

use crate::{AplRef, APLValue, KapNumber};
use num_bigint::BigInt;
use num_rational::BigRational;
use std::rc::Rc;

/// Backing storage for an array's elements. All immutable after construction.
#[derive(Debug, Clone)]
pub enum ArrayData {
    Long(Vec<i64>),
    Double(Vec<f64>),
    Char(Vec<char>),
    BigInt(Vec<BigInt>),
    Rational(Vec<BigRational>),
    /// Nested/boxed: each element is a full `APLValue` (used for mixed or
    /// higher-rank arrays). Wrapped in `Rc` so elements can be shared (D1).
    Nested(Vec<AplRef<APLValue>>),
}

/// A Kap array: shape (dimensions) + backing data. Immutable.
#[derive(Debug, Clone)]
pub struct KapArray {
    pub dimensions: Vec<usize>,
    pub data: ArrayData,
}

impl KapArray {
    pub fn new(dimensions: Vec<usize>, data: ArrayData) -> Self {
        KapArray { dimensions, data }
    }

    /// Total element count = product of dimensions (0 for the empty/rank-0 case is
    /// treated as 1 scalar slot to match Kap's 1-element rank-0 arrays).
    pub fn element_count(&self) -> usize {
        if self.dimensions.is_empty() {
            1
        } else {
            self.dimensions.iter().product()
        }
    }

    pub fn rank(&self) -> usize {
        self.dimensions.len()
    }

    /// Build a 1-D vector from a list of numbers (the common case in tests).
    pub fn from_numbers(nums: Vec<KapNumber>) -> Self {
        let len = nums.len();
        // Choose the most specific backing type that fits all elements.
        // Phase 1: if all are Long -> Long vec; if all Double -> Double; etc.
        // Otherwise fall back to Nested.
        if nums.iter().all(|n| matches!(n, KapNumber::Long(_))) {
            let v = nums.into_iter().map(|n| match n {
                KapNumber::Long(x) => x,
                _ => unreachable!(),
            }).collect();
            KapArray::new(vec![len], ArrayData::Long(v))
        } else if nums.iter().all(|n| matches!(n, KapNumber::Double(_))) {
            let v = nums.into_iter().map(|n| match n {
                KapNumber::Double(x) => x,
                _ => unreachable!(),
            }).collect();
            KapArray::new(vec![len], ArrayData::Double(v))
        } else {
            let v = nums.into_iter().map(|n| Rc::new(APLValue::Number(n))).collect();
            KapArray::new(vec![len], ArrayData::Nested(v))
        }
    }

    /// Uniform view: all elements as `APLValue`s, regardless of backing storage.
    /// Used by the evaluator/REPL which must handle every array kind.
    pub fn elements(&self) -> Vec<AplRef<APLValue>> {
        match &self.data {
            ArrayData::Long(v) => v.iter().map(|x| Rc::new(APLValue::Number(KapNumber::Long(*x)))).collect(),
            ArrayData::Double(v) => v.iter().map(|x| Rc::new(APLValue::Number(KapNumber::Double(*x)))).collect(),
            ArrayData::Char(v) => v.iter().map(|x| Rc::new(APLValue::Char(*x))).collect(),
            ArrayData::BigInt(v) => v.iter().map(|x| Rc::new(APLValue::Number(KapNumber::BigInt(x.clone())))).collect(),
            ArrayData::Rational(v) => v.iter().map(|x| Rc::new(APLValue::Number(KapNumber::Rational(x.clone())))).collect(),
            ArrayData::Nested(v) => v.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_count_and_rank() {
        let a = KapArray::new(vec![3, 4], ArrayData::Long(vec![0; 12]));
        assert_eq!(a.element_count(), 12);
        assert_eq!(a.rank(), 2);
        let s = KapArray::new(vec![], ArrayData::Long(vec![7]));
        assert_eq!(s.element_count(), 1);
        assert_eq!(s.rank(), 0);
    }

    #[test]
    fn from_numbers_long_vector() {
        let a = KapArray::from_numbers(vec![
            KapNumber::Long(1),
            KapNumber::Long(2),
            KapNumber::Long(3),
        ]);
        assert!(matches!(a.data, ArrayData::Long(_)));
        assert_eq!(a.dimensions, vec![3]);
    }

    #[test]
    fn nested_arrays_are_supported() {
        // APL/Kap arrays nest arbitrarily: an element can itself be an array.
        // ArrayData::Nested holds full APLValues, so depth is unbounded.
        let inner = KapArray::from_numbers(vec![KapNumber::Long(1), KapNumber::Long(2)]);
        let outer = KapArray::new(
            vec![2],
            ArrayData::Nested(vec![
                std::rc::Rc::new(APLValue::Number(KapNumber::Long(0))),
                std::rc::Rc::new(APLValue::Array(std::rc::Rc::new(inner))),
            ]),
        );
        assert!(matches!(outer.data, ArrayData::Nested(_)));
        assert_eq!(outer.element_count(), 2);
        // The second element is itself an array of 2 elements.
        if let ArrayData::Nested(elems) = &outer.data {
            assert!(matches!(elems[1].as_ref(), APLValue::Array(_)));
        }
    }
}
