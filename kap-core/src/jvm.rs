//! Tier-1 `jvm:` interop values (§11a) — dependency-free, no JVM required.
//!
//! Port of the value-representation half of Kotlin
//! `src/jvmMain/.../jvmmod/jvm-module.kt`: `JvmInstanceValue` (a boxed Java
//! object) restricted to what is emulatable without a live JVM — scalar
//! conversions (`toJvm*`), primitive/array classes (`findPrimitiveTypeClass`),
//! emulated primitive arrays (`createArrayInstance`/`arraySetElement`), and
//! nominal class references (`findClass`, Tier-1 allowlist only).
//!
//! Reflection (`findMethod/findConstructor/callMethod/createInstance/fromJvm/…`)
//! needs a live JVM and stays unregistered until §11a-Tier-2.
//!
//! Honesty notes (documented divergences, all forced by the missing JVM):
//! - `findClass` resolves a small allowlist (`java.lang.String`); anything
//!   else errors `Class not found: <name>` exactly like Kotlin's
//!   `ClassNotFoundException` branch. A nominal `ClassRef` carries no methods.
//! - Emulated arrays display as `[<descr> <elems…>]` (e.g. `[I 100 200]`),
//!   NOT Java's nondeterministic `[I@6d8a00e3` identity hash.
//! - `toJvm{Short,Int,Long,Byte,Char}` truncate fractional rationals via
//!   numerator/denominator division like Kotlin `Rational.asLong`; the port's
//!   `kap_long` helper would reject them (kept distinct on purpose).

use std::cell::RefCell;
use std::rc::Rc;

use crate::number::KapNumber;

/// A JVM primitive type (`FindPrimitiveTypeClassFunction.typeMap` keys).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JvmPrim {
    Char,
    Byte,
    Short,
    Int,
    Long,
    Float,
    Double,
    Boolean,
}

impl JvmPrim {
    /// `Class.getName()` for the primitive (`char`, `int`, …).
    pub fn get_name(self) -> &'static str {
        match self {
            JvmPrim::Char => "char",
            JvmPrim::Byte => "byte",
            JvmPrim::Short => "short",
            JvmPrim::Int => "int",
            JvmPrim::Long => "long",
            JvmPrim::Float => "float",
            JvmPrim::Double => "double",
            JvmPrim::Boolean => "boolean",
        }
    }

    /// `Class.getName()` for the array type (`[I`, `[Z`, …).
    pub fn array_name(self) -> &'static str {
        match self {
            JvmPrim::Char => "[C",
            JvmPrim::Byte => "[B",
            JvmPrim::Short => "[S",
            JvmPrim::Int => "[I",
            JvmPrim::Long => "[J",
            JvmPrim::Float => "[F",
            JvmPrim::Double => "[D",
            JvmPrim::Boolean => "[Z",
        }
    }

    /// Zero element for a fresh `createArrayInstance` array.
    pub fn zero(self) -> JvmScalar {
        match self {
            JvmPrim::Char => JvmScalar::Char('\0'),
            JvmPrim::Byte => JvmScalar::Byte(0),
            JvmPrim::Short => JvmScalar::Short(0),
            JvmPrim::Int => JvmScalar::Int(0),
            JvmPrim::Long => JvmScalar::Long(0),
            JvmPrim::Float => JvmScalar::Float(0.0),
            JvmPrim::Double => JvmScalar::Double(0.0),
            JvmPrim::Boolean => JvmScalar::Bool(false),
        }
    }
}

/// A class value: primitive, primitive array, or `void`
/// (`FindPrimitiveTypeClassFunction.typeMap` values).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JvmClass {
    Prim(JvmPrim),
    PrimArray(JvmPrim),
    Void,
}

/// Map a `'jvm:<name>` symbol to its class (`typeMap` in
/// `FindPrimitiveTypeClassFunctionImpl.init`, jvm-module.kt:285-306).
/// Unknown names are `None` → caller errors `Unexpected type name: …`.
pub fn prim_class_for_symbol(name: &str) -> Option<JvmClass> {
    if name == "void" {
        return Some(JvmClass::Void);
    }
    let p = match name {
        "char" => JvmPrim::Char,
        "byte" => JvmPrim::Byte,
        "short" => JvmPrim::Short,
        "int" => JvmPrim::Int,
        "long" => JvmPrim::Long,
        "float" => JvmPrim::Float,
        "double" => JvmPrim::Double,
        "boolean" => JvmPrim::Boolean,
        _ => {
            let a = match name {
                "charArray" => JvmPrim::Char,
                "byteArray" => JvmPrim::Byte,
                "shortArray" => JvmPrim::Short,
                "intArray" => JvmPrim::Int,
                "longArray" => JvmPrim::Long,
                "floatArray" => JvmPrim::Float,
                "doubleArray" => JvmPrim::Double,
                "booleanArray" => JvmPrim::Boolean,
                _ => return None,
            };
            return Some(JvmClass::PrimArray(a));
        }
    };
    if name == "void" {
        return Some(JvmClass::Void);
    }
    Some(JvmClass::Prim(p))
}

/// A boxed Java scalar (`JvmInstanceValue` over a primitive/String/byte[]).
#[derive(Clone, PartialEq, Debug)]
pub enum JvmScalar {
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Char(char),
    Str(String),
    Bytes(Vec<u8>),
}

/// A Tier-1/Tier-2 JVM value (`JvmInstanceValue`).
#[derive(Clone, PartialEq, Debug)]
pub enum JvmValue {
    Scalar(JvmScalar),
    Class(JvmClass),
    /// Nominal class reference (`findClass`, Tier-2 nominal: no classpath
    /// without a JVM, so every named class resolves).
    ClassRef(String),
    /// Nominal constructor reference (`findConstructor`). `sigs` holds the
    /// bound parameter class names (getName form); empty = nominal fallback
    /// (Tier-2) or no-arg.
    Ctor { class: String, sigs: Vec<String> },
    /// Nominal method reference (`findMethod`). `sigs`/`ret` (getName form)
    /// and `is_static` come from bind-time reflection; all-empty/false =
    /// nominal fallback (Tier-2).
    Method {
        class: String,
        name: String,
        sigs: Vec<String>,
        ret: String,
        is_static: bool,
    },
    /// Nominal field reference (`findField`). `ftype` (getName form) from
    /// bind-time reflection; empty = nominal fallback.
    Field {
        class: String,
        name: String,
        ftype: String,
        /// `Modifier.isStatic` — a static constant is read with
        /// `GetStatic<Type>Field` and a `null` instance (`field.get(null)`).
        is_static: bool,
    },
    /// Nominal instance (`createInstance`, Tier-2 nominal).
    Instance(String),
    /// Live JNI object (Tier-3 real bridge): process-global-ref id plus the
    /// class name (for display and `instanceof`-style checks). The ref itself
    /// lives in `jvmbridge`'s registry so the value stays `Clone + PartialEq`.
    Live { id: u64, class: String },
    /// Emulated primitive array (`createArrayInstance`); mutated in place by
    /// `arraySetElement` through the handle's `RefCell`.
    ObjectArray { elem: JvmPrim, data: Vec<JvmScalar> },
}

/// Shared handle (mirrors `Stream`/`Process`: `Rc<RefCell<…>>`, D1-safe).
pub type JvmHandle = Rc<RefCell<JvmValue>>;

pub fn new_jvm(v: JvmValue) -> JvmHandle {
    Rc::new(RefCell::new(v))
}

fn java_float_str(v: f64) -> String {
    // Java `Float.toString`/`Double.toString`: shortest round-trip with `.0`
    // on integral mantissas. The port's `format_double` already implements it.
    crate::number::format_double(v)
}

impl JvmValue {
    /// `JvmInstanceValue.formatted` (jvm-module.kt:74-76): `instance.toString()`.
    pub fn display(&self) -> String {
        match self {
            JvmValue::Scalar(s) => match s {
                JvmScalar::Bool(b) => b.to_string(),
                JvmScalar::Byte(b) => b.to_string(),
                JvmScalar::Short(n) => n.to_string(),
                JvmScalar::Int(n) => n.to_string(),
                JvmScalar::Long(n) => n.to_string(),
                JvmScalar::Float(f) => java_float_str(*f as f64),
                JvmScalar::Double(d) => java_float_str(*d),
                JvmScalar::Char(c) => c.to_string(),
                JvmScalar::Str(s) => s.clone(),
                JvmScalar::Bytes(b) => {
                    format!("[B {}]", b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "))
                }
            },
            JvmValue::Class(JvmClass::Prim(p)) => p.get_name().to_string(),
            JvmValue::Class(JvmClass::PrimArray(p)) => p.array_name().to_string(),
            JvmValue::Class(JvmClass::Void) => "void".to_string(),
            JvmValue::ClassRef(n) => format!("class {}", n),
            JvmValue::Ctor { class, .. } => format!("constructor {}", class),
            JvmValue::Method { class, name, .. } => format!("method {}.{}", class, name),
            JvmValue::Field { class, name, .. } => format!("field {}.{}", class, name),
            JvmValue::Instance(n) => format!("instance {}", n),
            JvmValue::Live { class, .. } => format!("instance {}", class),
            JvmValue::ObjectArray { elem, data } => {
                let desc = elem.array_name();
                let body = data.iter().map(|s| JvmValue::Scalar(s.clone()).display()).collect::<Vec<_>>().join(" ");
                if body.is_empty() {
                    desc.to_string()
                } else {
                    format!("{} {}]", desc, body)
                }
            }
        }
    }

    /// `Class.isInstance` over the emulated model (`InstanceOfFunction`).
    /// Only modeled pairs answer true; everything else is a genuine false
    /// (a nominal `ClassRef` has no hierarchy until Tier 2).
    pub fn instance_of(obj: &JvmScalar, ty: &JvmValue) -> bool {
        match ty {
            JvmValue::ClassRef(n) if n == "java.lang.String" => matches!(obj, JvmScalar::Str(_)),
            JvmValue::ClassRef(_) => false,
            JvmValue::Class(JvmClass::Prim(p)) => match (obj, p) {
                (JvmScalar::Char(_), JvmPrim::Char)
                | (JvmScalar::Byte(_), JvmPrim::Byte)
                | (JvmScalar::Short(_), JvmPrim::Short)
                | (JvmScalar::Int(_), JvmPrim::Int)
                | (JvmScalar::Long(_), JvmPrim::Long)
                | (JvmScalar::Float(_), JvmPrim::Float)
                | (JvmScalar::Double(_), JvmPrim::Double)
                | (JvmScalar::Bool(_), JvmPrim::Boolean) => true,
                _ => false,
            },
            JvmValue::Class(JvmClass::PrimArray(p)) => {
                matches!(obj, JvmScalar::Bytes(_)) && *p == JvmPrim::Byte
            }
            JvmValue::Class(JvmClass::Void) => false,
            _ => false,
        }
    }
}

/// `ensureNumber().asLong()` for the `toJvm{Short,Int,Long,Byte,Char}` family
/// (jvm-module.kt:374/388/402/413/427): doubles truncate (`toLong`), bigints
/// range-check, rationals truncate via numerator/denominator division.
/// Complex and non-numbers are errors.
pub fn jvm_long(n: &KapNumber) -> Result<i64, String> {
    match n {
        KapNumber::Long(v) => Ok(*v),
        KapNumber::Double(v) => Ok(*v as i64),
        KapNumber::BigInt(v) => crate::number::bigint_to_i64(v),
        KapNumber::Rational(v) => {
            let num = crate::number::bigint_to_i64(v.numer())?;
            let den = crate::number::bigint_to_i64(v.denom())?;
            Ok(num / den)
        }
        KapNumber::Complex(_, _) => Err("not an integer".to_string()),
    }
}

/// `toJava` for `arraySetElement` into a primitive array
/// (jvm-module.kt:126-201): longs range-check per target (`longToJava` —
/// note `Byte` accepts 0..255), doubles truncate (`doubleToJava`), bigints go
/// through long when they fit (`bigintToJava`). Rationals have no arm and
/// always error, exactly like Kotlin.
pub fn coerce_to_prim(n: &KapNumber, elem: JvmPrim) -> Result<JvmScalar, String> {
    let err = |what: &str| format!("Cannot convert to Java: {}", what);
    let fit_long = |v: i64| -> Result<i64, String> {
        let ok = match elem {
            JvmPrim::Byte => (0..=255).contains(&v),
            JvmPrim::Short => (i64::from(i16::MIN)..=i64::from(i16::MAX)).contains(&v),
            JvmPrim::Int => (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&v),
            JvmPrim::Long => true,
            JvmPrim::Float | JvmPrim::Double => true,
            JvmPrim::Char => (0..=0xffff).contains(&v),
            JvmPrim::Boolean => false,
        };
        if ok {
            Ok(v)
        } else {
            Err(err(&v.to_string()))
        }
    };
    let store = |v: i64| -> Result<JvmScalar, String> {
        let v = fit_long(v)?;
        Ok(match elem {
            JvmPrim::Byte => JvmScalar::Byte(v as u8 as i8),
            JvmPrim::Short => JvmScalar::Short(v as i16),
            JvmPrim::Int => JvmScalar::Int(v as i32),
            JvmPrim::Long => JvmScalar::Long(v),
            JvmPrim::Float => JvmScalar::Float(v as f32),
            JvmPrim::Double => JvmScalar::Double(v as f64),
            JvmPrim::Char => JvmScalar::Char(char::from_u32(v as u32).unwrap_or('\0')),
            JvmPrim::Boolean => return Err(err(&v.to_string())),
        })
    };
    match n {
        KapNumber::Long(v) => store(*v),
        KapNumber::Double(v) => match elem {
            JvmPrim::Float => Ok(JvmScalar::Float(*v as f32)),
            JvmPrim::Double => Ok(JvmScalar::Double(*v)),
            _ => store(*v as i64),
        },
        KapNumber::BigInt(v) => match crate::number::bigint_to_i64(v) {
            Ok(l) => store(l),
            Err(_) => Err(err(&v.to_string())),
        },
        _ => Err(err("rational")),
    }
}
