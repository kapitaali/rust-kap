//! P2 port of Kotlin `encoder/encoder.kt`: the Kap binary value wire format.
//!
//! Format (EncoderConstants):
//! - Header: 0x99 'k' 'a' 'p'
//! - Type indicator byte; bit 7 (0x80) = array, else atom:
//!   - atoms (bits 3–6 select type): compressed-int(11<<3), int8(0), dynint64(2<<3),
//!     double(3<<3), complex(4<<3), bigint(5<<3), rational(6<<3), char8(7<<3),
//!     char32(8<<3), nil(9<<3), enclosed(10<<3), map/list/symbol/generic
//!   - arrays: 0x80 | labels(0x40) | rank-in-low-6-bits (63 => rank follows as dynint64)
//! - dynint64: 7-bit groups, little-endian-first, continuation bit 0x80;
//!   complement bit 0x40 on the first byte for negatives.

use crate::array::{ArrayData, KapArray};
use crate::number::KapNumber;
use crate::APLValue;
use std::rc::Rc;

const HEADER: [u8; 4] = [0x99, b'k', b'a', b'p'];

const T_INT8: u8 = 0;
const T_DYNINT64: u8 = 2 << 3;
const T_DOUBLE: u8 = 3 << 3;
const T_COMPLEX: u8 = 4 << 3;
const T_BIGINT: u8 = 5 << 3;
const T_RATIONAL: u8 = 6 << 3;
const T_CHAR8: u8 = 7 << 3;
const T_CHAR32: u8 = 8 << 3;
const T_NIL: u8 = 9 << 3;
const T_ENCLOSED: u8 = 10 << 3;
const T_COMPRESSED_INT: u8 = 11 << 3;
const T_LIST: u8 = 13 << 3;
const T_SYMBOL: u8 = 14 << 3;
const ARRAY_TYPE_MASK: u8 = 0x80;
const ARRAY_LABELS_FLAG: u8 = 0x40;
const ARRAY_LENGTH_RANGE: u8 = 0x3f;

type EncResult<T> = Result<T, String>;

// ---------------- Encoder ----------------

fn write_dynint64(out: &mut Vec<u8>, value: i64) {
    if value == 0 {
        out.push(0);
        return;
    }
    // Kotlin writes `value.toULong().inv()` for negatives (full bitwise NOT).
    let v: u64 = if value < 0 { !(value as u64) } else { value as u64 };
    let mut n = 8usize;
    let mut mask: u64 = 0x7f00000000000000;
    while n > 0 && (v & mask) == 0 {
        mask >>= 7;
        n -= 1;
    }
    let mut needs_inverse_tag = true;
    if v & (0x40u64 << (n * 7)) != 0 {
        out.push(if value < 0 { 0xc0 } else { 0x80 });
        needs_inverse_tag = false;
    }
    loop {
        let block = ((v & mask) >> (7 * n)) as u8;
        let adjusted = block
            | (if n == 0 { 0 } else { 0x80 })
            | (if needs_inverse_tag && value < 0 { 0x40 } else { 0 });
        out.push(adjusted);
        mask >>= 7;
        if n == 0 {
            break;
        }
        n -= 1;
        needs_inverse_tag = false;
    }
}

fn write_long(out: &mut Vec<u8>, value: i64) {
    if (-4..=3).contains(&value) {
        out.push(T_COMPRESSED_INT | ((value as u8) & 0x7));
    } else if (-128..=127).contains(&value) {
        out.push(T_INT8);
        out.push(value as i8 as u8);
    } else {
        out.push(T_DYNINT64);
        write_dynint64(out, value);
    }
}

fn write_double_bits(out: &mut Vec<u8>, value: f64) {
    out.push(T_DOUBLE);
    out.extend_from_slice(&value.to_bits().to_be_bytes());
}

fn write_char(out: &mut Vec<u8>, c: char) {
    let cp = c as u32;
    if cp <= 255 {
        out.push(T_CHAR8);
        out.push(cp as u8);
    } else {
        out.push(T_CHAR32);
        out.extend_from_slice(&cp.to_be_bytes());
    }
}

fn write_bigint(out: &mut Vec<u8>, v: &num_bigint::BigInt) {
    use num_traits::ToPrimitive;
    match v.to_i64() {
        Some(l) => write_long(out, l),
        None => {
            out.push(T_BIGINT);
            let buf = v.to_signed_bytes_be();
            write_dynint64(out, buf.len() as i64);
            out.extend_from_slice(&buf);
        }
    }
}

fn write_rational(out: &mut Vec<u8>, r: &num_rational::BigRational) {
    if r.denom() == &num_bigint::BigInt::from(1) {
        write_bigint(out, r.numer());
    } else {
        out.push(T_RATIONAL);
        write_bigint(out, r.numer());
        write_bigint(out, r.denom());
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    write_dynint64(out, s.len() as i64);
    out.extend_from_slice(s.as_bytes());
}

/// Encode a value without the header. `enclose` marks an already-enclosed scalar.
fn write_value(out: &mut Vec<u8>, v: &APLValue) -> EncResult<()> {
    match v {
        APLValue::Number(KapNumber::Long(l)) => write_long(out, *l),
        APLValue::Number(KapNumber::Double(d)) => write_double_bits(out, *d),
        APLValue::Number(KapNumber::BigInt(b)) => write_bigint(out, b),
        APLValue::Number(KapNumber::Rational(r)) => write_rational(out, r),
        APLValue::Number(KapNumber::Complex(re, im)) => {
            out.push(T_COMPLEX);
            out.extend_from_slice(&re.to_bits().to_be_bytes());
            out.extend_from_slice(&im.to_bits().to_be_bytes());
        }
        APLValue::Char(c) => write_char(out, *c),
        // Kotlin models a Kap string as a rank-1 CHAR array: "abc" encodes as
        // [array-header(1), len, char8 'a', char8 'b', char8 'c'].
        APLValue::Str(s) => {
            out.push(ARRAY_TYPE_MASK | 1);
            write_dynint64(out, s.chars().count() as i64);
            for c in s.chars() {
                write_char(out, c);
            }
        }
        APLValue::Null => out.push(T_NIL),
        APLValue::Symbol { name, namespace } => {
            out.push(T_SYMBOL);
            write_string(out, namespace.as_deref().unwrap_or("default"));
            write_string(out, name);
        }
        APLValue::Array(a) => {
            // The port models nested vectors and enclosures as rank-1/rank-0 arrays.
            let dims = &a.dimensions;
            if dims.len() >= ARRAY_LENGTH_RANGE as usize {
                out.push(ARRAY_TYPE_MASK | ARRAY_LENGTH_RANGE);
                write_dynint64(out, dims.len() as i64);
            } else {
                out.push(ARRAY_TYPE_MASK | dims.len() as u8);
            }
            for d in dims.iter() {
                write_dynint64(out, *d as i64);
            }
            for e in a.elements() {
                write_value(out, e.as_ref())?;
            }
        }
        other => return Err(format!("Unable to encode values of type: {}", type_name(other))),
    }
    Ok(())
}

fn type_name(v: &APLValue) -> &'static str {
    match v {
        APLValue::Str(_) => "string",
        _ => "value",
    }
}

pub fn encode_value(v: &APLValue) -> EncResult<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&HEADER);
    write_value(&mut out, v)?;
    Ok(out)
}

// ---------------- Decoder ----------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> EncResult<u8> {
        self.buf
            .get(self.pos)
            .copied()
            .ok_or_else(|| "End of stream while decoding long".to_string())
            .map(|b| {
                self.pos += 1;
                b
            })
    }
    fn take(&mut self, n: usize) -> EncResult<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            return Err("Attempt to read past end of stream".into());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn dynint64(&mut self) -> EncResult<i64> {
        let first = self.byte()?;
        let mut res: i64 = (first & 0x3f) as i64;
        let mut needs_more = (first & 0x80) != 0;
        while needs_more {
            let nb = self.byte()?;
            res = (res << 7) | (nb & 0x7f) as i64;
            needs_more = (nb & 0x80) != 0;
        }
        Ok(if first & 0x40 != 0 { !res } else { res })
    }
    fn length(&mut self) -> EncResult<usize> {
        let n = self.dynint64()?;
        if !(0..=i32::MAX as i64).contains(&n) {
            return Err(format!("Invalid length: {}", n));
        }
        Ok(n as usize)
    }
}

fn read_value(r: &mut Reader) -> EncResult<APLValue> {
    let t = r.byte()?;
    if t & 0x80 == 0 {
        read_atom(r, t)
    } else {
        read_array(r, t)
    }
}

fn read_atom(r: &mut Reader, t: u8) -> EncResult<APLValue> {
    let kind = t & !0x07 & 0xf8; // bits 3-6 handled below via full compare instead
    let _ = kind;
    let ti = t & 0xf8 & 0x7f;
    // Match against our constants directly on the low 7 bits minus array flag.
    let base = t & 0x78 | (t & 0x07);
    let _ = base;
    if t & 0x78 == T_COMPRESSED_INT & 0x78 {
        // Compressed int carries its value in bits 0-2.
        let v = ((t & 0x7) as i8 as i64) >> 5; // sign-extend 3 bits like Kotlin shl/shr dance
        let v = ((t & 0x7) as i64).sign_extend_3();
        return Ok(num(v));
    }
    match ti_of(t) {
        k if k == T_INT8 => Ok(num(r.byte()? as i64)),
        k if k == T_DYNINT64 => Ok(num(r.dynint64()?)),
        k if k == T_BIGINT => {
            let len = r.dynint64()?;
            let buf = r.take(len as usize)?;
            Ok(APLValue::Number(KapNumber::BigInt(
                num_bigint::BigInt::from_signed_bytes_be(buf),
            )))
        }
        k if k == T_DOUBLE => {
            let bits = u64::from_be_bytes(r.take(8)?.try_into().unwrap());
            Ok(APLValue::Number(KapNumber::Double(f64::from_bits(bits))))
        }
        k if k == T_COMPLEX => {
            let re = f64::from_bits(u64::from_be_bytes(r.take(8)?.try_into().unwrap()));
            let im = f64::from_bits(u64::from_be_bytes(r.take(8)?.try_into().unwrap()));
            Ok(APLValue::Number(KapNumber::Complex(re, im)))
        }
        k if k == T_RATIONAL => {
            let numer = read_kap_integer(r)?;
            let denom = read_kap_integer(r)?;
            Ok(APLValue::Number(KapNumber::Rational(
                num_rational::BigRational::new(numer, denom),
            )))
        }
        k if k == T_CHAR8 => {
            let b = r.byte()?;
            Ok(char::from_u32(b as u32)
                .map(APLValue::Char)
                .unwrap_or(APLValue::Null))
        }
        k if k == T_CHAR32 => {
            let cp = u32::from_be_bytes(r.take(4)?.try_into().unwrap());
            Ok(char::from_u32(cp).map(APLValue::Char).unwrap_or(APLValue::Null))
        }
        k if k == T_NIL => Ok(APLValue::Null),
        k if k == T_ENCLOSED => {
            let v = read_value(r)?;
            Ok(APLValue::Array(Rc::new(KapArray::new(
                vec![],
                ArrayData::Nested(vec![Rc::new(v)]),
            ))))
        }
        k if k == T_LIST => {
            let n = r.length()?;
            let mut elems = Vec::with_capacity(n);
            for _ in 0..n {
                elems.push(Rc::new(read_value(r)?));
            }
            Ok(APLValue::Array(Rc::new(KapArray::new(
                vec![n],
                ArrayData::Nested(elems),
            ))))
        }
        k if k == T_SYMBOL => {
            let ns_len = r.dynint64()? as usize;
            let ns = String::from_utf8_lossy(r.take(ns_len)?).to_string();
            let name_len = r.dynint64()? as usize;
            let nm = String::from_utf8_lossy(r.take(name_len)?).to_string();
            Ok(APLValue::Symbol {
                name: nm,
                namespace: if ns == "default" { None } else { Some(ns) },
            })
        }
        _ => Err(format!("Unexpected type: {}", ti)),
    }
}

trait SignExtend3 {
    fn sign_extend_3(self) -> i64;
}
impl SignExtend3 for i64 {
    fn sign_extend_3(self) -> i64 {
        (self ^ 0x4).wrapping_sub(0x4)
    }
}

fn num(v: i64) -> APLValue {
    APLValue::Number(KapNumber::Long(v))
}

fn ti_of(t: u8) -> u8 {
    // Mask off the value bits used by compressed ints (bits 0-2).
    t & 0xf8 & 0xff
}

fn read_kap_integer(r: &mut Reader) -> EncResult<num_bigint::BigInt> {
    let b = r.byte()?;
    let v = read_atom(r, b)?;
    match v {
        APLValue::Number(n) => match n {
            KapNumber::Long(l) => Ok(num_bigint::BigInt::from(l)),
            KapNumber::BigInt(b) => Ok(b),
            _ => Err("Expected integer in rational".into()),
        },
        _ => Err("Expected integer in rational".into()),
    }
}

fn read_array(r: &mut Reader, t: u8) -> EncResult<APLValue> {
    let rank_hdr = t & ARRAY_LENGTH_RANGE;
    let rank = if rank_hdr == ARRAY_LENGTH_RANGE {
        r.length()?
    } else {
        rank_hdr as usize
    };
    let mut dims = Vec::with_capacity(rank);
    for _ in 0..rank {
        dims.push(r.length()? as usize);
    }
    if t & ARRAY_LABELS_FLAG != 0 {
        return Err("Decoding of labels not supported".into());
    }
    let count: usize = dims.iter().product();
    let mut elems = Vec::with_capacity(count);
    for _ in 0..count {
        elems.push(Rc::new(read_value(r)?));
    }
    Ok(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(elems)))))
}

pub fn decode_value(bytes: &[u8]) -> EncResult<APLValue> {
    if bytes.len() < 4 || bytes[..4] != HEADER {
        return Err("Header mismatch".into());
    }
    let mut r = Reader { buf: bytes, pos: 4 };
    read_value(&mut r)
}

#[allow(dead_code)]
fn unused(_: u8) {
    let _ = (T_LIST, T_SYMBOL, ARRAY_TYPE_MASK);
}
