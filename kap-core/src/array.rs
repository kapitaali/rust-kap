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

/// A single axis label. `None` means "no label for this position" (Kotlin
/// `AxisLabel?` — null entries are explicit gaps, distinct from an empty-string
/// label which is the default for unlabelled axes).
pub type AxisLabel = Option<String>;

/// Labels for every axis of an array, in axis order. `labels[axis]` is
/// `None` when that axis has no labels at all; `Some(list)` where each
/// element is `Some(title)` or `None` (gap) when the axis is labelled.
#[derive(Debug, Clone)]
pub struct DimensionLabels {
    /// One entry per axis; each entry is None (no labels) or Some(per-position labels).
    pub labels: Vec<Option<Vec<AxisLabel>>>,
}

impl DimensionLabels {
    /// Create labels for `rank` axes, all unlabelled.
    pub fn none(rank: usize) -> Self {
        DimensionLabels { labels: vec![None; rank] }
    }

    /// Compute which axes have at least one non-null label (Kotlin
    /// `DimensionLabels.computeLabelledAxes()`).
    pub fn compute_labelled_axes(&self) -> Vec<bool> {
        self.labels
            .iter()
            .map(|axis| match axis {
                None => false,
                Some(list) => list.iter().any(|l| l.is_some()),
            })
            .collect()
    }

    /// Insert a new axis at position `axis` (Kotlin `insertAxis`). The new
    /// axis starts unlabelled; axes at and above `axis` shift up by one.
    pub fn insert_axis(&self, axis: usize) -> Self {
        let mut new = Vec::with_capacity(self.labels.len() + 1);
        for (i, l) in self.labels.iter().enumerate() {
            if i == axis {
                new.push(None);
            }
            new.push(l.clone());
        }
        if axis >= self.labels.len() {
            new.push(None);
        }
        DimensionLabels { labels: new }
    }

    /// Delete axis `axis` (Kotlin `deleteAxis`). Axes above shift down.
    pub fn delete_axis(&self, axis: usize) -> Self {
        let mut new = Vec::with_capacity(self.labels.len().saturating_sub(1));
        for (i, l) in self.labels.iter().enumerate() {
            if i != axis {
                new.push(l.clone());
            }
        }
        DimensionLabels { labels: new }
    }

    /// Reverse the labels on a single axis (used by `⌽`/`⊖` monadic).
    pub fn reverse_axis(&self, axis: usize) -> Self {
        let mut new = self.labels.clone();
        if let Some(Some(list)) = new.get_mut(axis) {
            list.reverse();
        }
        DimensionLabels { labels: new }
    }

    /// Rotate labels on a single axis by `n` positions (used by dyadic `⌽`/`⊖`).
    pub fn rotate_axis(&self, axis: usize, n: isize, axis_size: usize) -> Self {
        let mut new = self.labels.clone();
        if let Some(Some(list)) = new.get_mut(axis) {
            let len = list.len();
            if len > 0 {
                let n = ((n as isize).rem_euclid(len as isize)) as usize;
                let mut rotated = Vec::with_capacity(len);
                for i in 0..len {
                    rotated.push(list[(i + len - n) % len].clone());
                }
                *list = rotated;
            }
        }
        DimensionLabels { labels: new }
    }

    /// Take `n` elements from the front of `axis`, keeping only their labels.
    pub fn take_axis(&self, axis: usize, n: usize) -> Self {
        let mut new = self.labels.clone();
        if let Some(Some(list)) = new.get_mut(axis) {
            list.truncate(n);
        }
        DimensionLabels { labels: new }
    }

    /// Drop `n` elements from the front of `axis`.
    pub fn drop_axis(&self, axis: usize, n: usize) -> Self {
        let mut new = self.labels.clone();
        if let Some(Some(list)) = new.get_mut(axis) {
            if n >= list.len() {
                *list = Vec::new();
            } else {
                *list = list[n..].to_vec();
            }
        }
        DimensionLabels { labels: new }
    }

    /// Take with fill: extend labels with `null` for padding positions.
    pub fn take_with_fill(&self, axis: usize, n: usize, axis_size: usize, fill_left: bool) -> Self {
        let mut new = self.labels.clone();
        if let Some(Some(list)) = new.get_mut(axis) {
            let mut result = Vec::with_capacity(n);
            if fill_left {
                let pad = n.saturating_sub(axis_size);
                for _ in 0..pad {
                    result.push(None);
                }
                for i in 0..axis_size.min(n) {
                    result.push(list[i].clone());
                }
            } else {
                let pad = n.saturating_sub(axis_size);
                for i in 0..axis_size.min(n) {
                    if i + pad < n {
                        // skip padding on the right
                    }
                }
                // Right-aligned: take last n, pad left with null
                let start = axis_size.saturating_sub(n);
                for _ in 0..(n - axis_size.min(n)) {
                    result.push(None);
                }
                for i in start..axis_size {
                    result.push(list[i].clone());
                }
            }
            *list = result;
        }
        DimensionLabels { labels: new }
    }
}

/// A Kap array: shape (dimensions) + backing data + optional axis labels. Immutable.
#[derive(Debug, Clone)]
pub struct KapArray {
    pub dimensions: Vec<usize>,
    pub data: ArrayData,
    /// Axis labels (Kap `DimensionLabels`). `None` when no axis is labelled.
    pub labels: Option<Box<DimensionLabels>>,
}

impl KapArray {
    pub fn new(dimensions: Vec<usize>, data: ArrayData) -> Self {
        KapArray { dimensions, data, labels: None }
    }

    /// Create a labelled array. `labels` is one entry per axis; each entry
    /// is `Some(per-position)` where positions are `Some(title)` or `None` (gap).
    pub fn with_labels(dimensions: Vec<usize>, data: ArrayData, labels: Vec<Option<Vec<AxisLabel>>>) -> Self {
        KapArray { dimensions, data, labels: Some(Box::new(DimensionLabels { labels })) }
    }

    /// Get a reference to the labels, if any.
    pub fn labels(&self) -> Option<&DimensionLabels> {
        self.labels.as_deref()
    }

    /// Clone with new labels.
    pub fn clone_with_labels(&self, labels: Option<Box<DimensionLabels>>) -> Self {
        KapArray { dimensions: self.dimensions.clone(), data: self.data.clone(), labels }
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
