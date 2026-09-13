//! Tier-3 real JNI bridge (`jvm:` namespace, jvmmod/jvm-module.kt).
//!
//! The Tier-2 nominal layer (`jvm.rs`) binds class/method/field *tokens* but
//! cannot execute anything, because it has no JVM. This module adds the real
//! thing: it starts a JVM in-process (via the `jni` crate's `invocation`
//! feature, which dlopens libjvm at RUNTIME), resolves members by reflection,
//! and invokes them, converting values both ways.
//!
//! Design rules:
//! - **Optional**: every entry point returns `None` when no JVM could be
//!   started (no `JAVA_HOME`, no libjvm, or an already-broken JVM). Callers
//!   fall back to the Tier-2 nominal behaviour, so the binary still runs on a
//!   machine with no JDK.
//! - **No panics**: JNI errors become `Err(String)`; a Java exception becomes
//!   `Err(JErr::Thrown { .. })` so the evaluator can raise the catchable
//!   `jvm:jvmMethodCallException` tag.
//! - **Live objects** are process-global JNI refs held in a registry and
//!   referred to by `u64` id, so `APLValue` stays `Clone + PartialEq`.
//!
//! Discovery: `JAVA_HOME` locates the JVM (the `jni` crate searches it, then
//! `PATH`/`LD_LIBRARY_PATH`). `KAP_JVM_CLASSPATH` adds application classes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue};
use jni::{InitArgsBuilder, JNIEnv, JNIVersion, JavaVM};

/// A Kap-side argument on its way into Java.
#[derive(Debug, Clone, PartialEq)]
pub enum JArg {
    Null,
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Char(char),
    Str(String),
    /// Live object id (see [`register`]).
    Obj(u64),
    Bytes(Vec<u8>),
    /// A Kap numeric vector. The target parameter's descriptor decides the
    /// Java array type (`[B`/`[S`/`[I`/`[J`/`[D`), mirroring Kotlin `toJava`'s
    /// type-directed array conversion — `xml.kap`'s `readString` passes the
    /// `String.getBytes` result straight to the `ByteArrayInputStream(byte[])`
    /// constructor.
    Longs(Vec<i64>),
}

/// A Java-side result on its way back into Kap.
#[derive(Debug, Clone, PartialEq)]
pub enum JVal {
    /// Java `null`.
    Null,
    /// A `void` method's result (`null` in Kotlin — `JvmInstanceValue(null)`).
    Void,
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Char(char),
    Str(String),
    /// Live object (registered global ref).
    Obj { id: u64, class: String },
    Bytes(Vec<u8>),
    /// A value whose Java type has no Kap mapping (e.g. a `java.util.List`).
    /// Carried so the evaluator can raise Kotlin's
    /// "Unexpected JVM type: <name>" from `javaObjToKap`.
    Other { class: String },
}

/// A Java exception raised by an invoked member
/// (Kotlin `JvmFunctionCallException` → the `jvm:jvmMethodCallException` tag).
#[derive(Debug, Clone, PartialEq)]
pub struct JThrown {
    pub class: String,
    pub message: String,
    /// Registry id of a global ref to the throwable itself (`0` when the
    /// object could not be registered). Kotlin throws
    /// `JvmInstanceValue(originException)` as the tag data, so handlers like
    /// `xml.kap`'s can call `jvm:callMethod⟦getMessageMethod; ⍺⟧` on the
    /// caught exception.
    pub id: u64,
}

/// Bridge failure: either a Java exception (catchable) or a bridge-level error.
#[derive(Debug, Clone, PartialEq)]
pub enum JErr {
    Thrown(JThrown),
    Message(String),
}

/// Reflection binding for a method (`findMethod`).
#[derive(Debug, Clone, PartialEq)]
pub struct MethodBind {
    /// Parameter type names in `Class.getCanonicalName` form.
    pub params: Vec<String>,
    /// Return type in canonical-name form.
    pub ret: String,
    pub is_static: bool,
}

/// Reflection binding for a constructor (`findConstructor`).
#[derive(Debug, Clone, PartialEq)]
pub struct CtorBind {
    pub params: Vec<String>,
}

/// Reflection binding for a field (`findField`).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldBind {
    pub ftype: String,
    pub is_static: bool,
}

// ---------------------------------------------------------------- lifecycle

/// The process-wide JVM, started on first use. `None` (cached) = unavailable.
fn vm() -> Option<&'static JavaVM> {
    static VM: OnceLock<Option<JavaVM>> = OnceLock::new();
    VM.get_or_init(start_vm).as_ref()
}

fn start_vm() -> Option<JavaVM> {
    let mut builder = InitArgsBuilder::new().version(JNIVersion::V8);
    // Application classes: the JVM cannot see Kap's fixtures (or any user
    // class) unless they are on the classpath. `JAVA_HOME` finds the JVM
    // itself; this variable adds classes.
    if let Ok(cp) = std::env::var("KAP_JVM_CLASSPATH") {
        if !cp.is_empty() {
            // Leak the option string: `InitArgsBuilder::option` borrows it for
            // the builder's lifetime, and one small string per process is free.
            let opt: &'static str =
                Box::leak(format!("-Djava.class.path={}", cp).into_boxed_str());
            builder = builder.option(opt);
        }
    }
    let args = builder.build().ok()?;
    JavaVM::new(args).ok()
}

/// Whether a JVM is available (starts it on first call).
pub fn available() -> bool {
    vm().is_some()
}

/// Run `f` with an attached `JNIEnv`. `None` = no JVM available.
///
/// The caller must be the thread that wants to use the JVM: we attach the
/// CURRENT thread (the engine is single-threaded, and each conformance case
/// runs on its own worker thread, so attaching per call is both correct and
/// cheap).
fn with_env<T>(f: impl FnOnce(&mut JNIEnv) -> Result<T, JErr>) -> Option<Result<T, JErr>> {
    let vm = vm()?;
    let mut env = match vm.attach_current_thread() {
        Ok(e) => e,
        Err(e) => return Some(Err(JErr::Message(format!("JVM attach failed: {}", e)))),
    };
    let r = f(&mut env);
    // Never leave a pending exception behind: subsequent JNI entry points are
    // silent no-ops while one is pending, which would corrupt later calls.
    if env.exception_check().unwrap_or(false) {
        if std::env::var_os("KAP_JVM_DEBUG").is_some() {
            eprintln!(
                "JVM DEBUG: pending exception left by bridge call\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        let _ = env.exception_clear();
    }
    Some(r)
}

// ------------------------------------------------------------------ helpers

/// `java.lang.String` → `java/lang/String` (JNI's `FindClass` form).
fn slashed(name: &str) -> String {
    name.replace('.', "/")
}

/// JNI field/method descriptor for a canonical type name.
///
/// Reflection reports `int`, `java.lang.String`, `int[]`, `java.lang.String[]`;
/// JNI wants `I`, `Ljava/lang/String;`, `[I`, `[Ljava/lang/String;`.
fn descriptor_of(canonical: &str) -> Result<String, JErr> {
    let bad = |n: &str| Err(JErr::Message(format!("Unexpected JVM type: {}", n)));
    match canonical {
        "void" => Ok("V".into()),
        "boolean" => Ok("Z".into()),
        "byte" => Ok("B".into()),
        "char" => Ok("C".into()),
        "short" => Ok("S".into()),
        "int" => Ok("I".into()),
        "long" => Ok("J".into()),
        "float" => Ok("F".into()),
        "double" => Ok("D".into()),
        "" => bad(canonical),
        other => {
            // Arrays: `X[]` → `[` + descriptor(X). JNI's own `getName` form
            // for arrays is `[I` / `[Ljava.lang.String;`, so accept both.
            if let Some(inner) = other.strip_suffix("[]") {
                let d = descriptor_of(inner)?;
                Ok(format!("[{}", d))
            } else if let Some(rest) = other.strip_prefix('[') {
                // Already JNI-ish (`[I`, `[Ljava.lang.String;`).
                let d = descriptor_of(rest.trim_end_matches(';'))?;
                Ok(format!("[{}", d))
            } else if other.starts_with("Ljava.") || other.starts_with("L") && other.ends_with(';') {
                Ok(other.to_string())
            } else {
                Ok(format!("L{};", slashed(other)))
            }
        }
    }
}

/// Java class name of a live object (`Class.getName`).
fn class_name_of(env: &mut JNIEnv, obj: &JObject) -> Result<String, JErr> {
    let clazz = env.get_object_class(obj).map_err(|e| {
        if std::env::var_os("KAP_JVM_DEBUG").is_some() {
            eprintln!("JVM DEBUG: getObjectClass failed: {}", e);
        }
        JErr::Message(format!("getObjectClass failed: {}", e))
    })?;
    let name = env
        .call_method(&clazz, "getName", "()Ljava/lang/String;", &[])
        .map_err(|e| {
            if std::env::var_os("KAP_JVM_DEBUG").is_some() {
                eprintln!("JVM DEBUG: Class.getName failed: {}", e);
            }
            JErr::Message(format!("getName failed: {}", e))
        })?;
    let s = name
        .l()
        .map_err(|e| JErr::Message(format!("getName result: {}", e)))?;
    Ok(jstring_to_string(env, &JString::from(s))?)
}

fn jstring_to_string(env: &mut JNIEnv, s: &JString) -> Result<String, JErr> {
    env.get_string(s)
        .map(|js| js.into())
        .map_err(|e| JErr::Message(format!("string conversion failed: {}", e)))
}

fn java_string<'a>(env: &mut JNIEnv<'a>, s: &str) -> Result<JString<'a>, JErr> {
    env.new_string(s)
        .map_err(|e| JErr::Message(format!("newString failed: {}", e)))
}

/// Convert a pending Java exception into [`JErr::Thrown`] (clearing it).
///
/// Order matters: JNI entry points are silent no-ops while an exception is
/// pending, so `getClass`/`getMessage` can only be called AFTER
/// `ExceptionClear`. The throwable local ref stays valid once cleared (the
/// spike proved this: clearing first is what makes the message readable).
fn take_pending(env: &mut JNIEnv) -> Option<JErr> {
    let throwable = env.exception_occurred().ok()?;
    if throwable.is_null() {
        return None;
    }
    if std::env::var_os("KAP_JVM_DEBUG").is_some() {
        static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "JVM DEBUG: take_pending (first) called from\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
    }
    let obj = JObject::from(throwable);
    if env.exception_clear().is_err() {
        return Some(JErr::Message("Java exception (unreadable)".into()));
    }
    if std::env::var_os("KAP_JVM_DEBUG").is_some() {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 {
            eprintln!(
                "JVM DEBUG: after clear #{}: check={:?} occurred={:?}",
                n,
                env.exception_check(),
                env.exception_occurred().map(|t| t.is_null())
            );
        }
    }
    let class = match class_name_of(env, &obj) {
        Ok(c) => c,
        Err(_) => "java.lang.Throwable".into(),
    };
    let raw = env
        .call_method(&obj, "getMessage", "()Ljava/lang/String;", &[])
        .ok()
        .and_then(|v| v.l().ok())
        .and_then(|o| {
            if o.is_null() {
                None
            } else {
                jstring_to_string(env, &JString::from(o)).ok()
            }
        })
        .unwrap_or_default();
    // `NoSuchFieldException`/`NoSuchMethodException` carry their detail in
    // `toString()` but a NULL `getMessage()`, so fall back to `toString()`
    // (Kotlin's handler interpolates the same shapes over `getMessage`).
    let message = if raw.is_empty() {
        call_string_method(env, &obj, "toString", "()Ljava/lang/String;").unwrap_or_default()
    } else {
        raw
    };
    // Keep the throwable itself alive (Kotlin's tag carries
    // `JvmInstanceValue(originException)`), so a handler can inspect it.
    let id = register(env, &obj).unwrap_or(0);
    // Drop any secondary throwable raised by the calls above (a clear with
    // nothing pending is a no-op).
    let _ = env.exception_clear();
    Some(JErr::Thrown(JThrown { class, message, id }))
}

// ----------------------------------------------------------- object registry

/// Live JNI objects held as process-global refs, addressed by id.
static REGISTRY: OnceLock<Mutex<HashMap<u64, (GlobalRef, String)>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<HashMap<u64, (GlobalRef, String)>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Promote a local object reference to a registered global ref, returning its id.
fn register(env: &mut JNIEnv, obj: &JObject) -> Result<u64, JErr> {
    if obj.is_null() {
        return Err(JErr::Message("cannot register a null object".into()));
    }
    let class = class_name_of(env, obj)?;
    let gref = env
        .new_global_ref(obj)
        .map_err(|e| JErr::Message(format!("newGlobalRef failed: {}", e)))?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    registry()
        .lock()
        .map_err(|_| JErr::Message("JVM registry poisoned".into()))?
        .insert(id, (gref, class));
    Ok(id)
}

/// A fresh LOCAL reference to a registered object (the registry holds a
/// global ref; JNI calls need a local one, and `JObject` is not `Clone`).
fn local_ref<'a>(env: &mut JNIEnv<'a>, id: u64) -> Result<JObject<'a>, JErr> {
    let reg = registry()
        .lock()
        .map_err(|_| JErr::Message("JVM registry poisoned".into()))?;
    let (gref, _) = reg
        .get(&id)
        .ok_or_else(|| JErr::Message(format!("unknown JVM object: {}", id)))?;
    let gref_obj: &JObject = gref.as_ref();
    env.new_local_ref(gref_obj)
        .map_err(|e| JErr::Message(format!("newLocalRef failed: {}", e)))
}

/// Run `f` with the object behind `id` (no JVM call: the ref is in the table).
fn with_object<T>(id: u64, f: impl FnOnce(&JObject) -> Result<T, JErr>) -> Result<T, JErr> {
    let reg = registry()
        .lock()
        .map_err(|_| JErr::Message("JVM registry poisoned".into()))?;
    match reg.get(&id) {
        Some((gref, _)) => f(gref.as_ref()),
        None => Err(JErr::Message(format!("unknown JVM object: {}", id))),
    }
}

/// Class name recorded for a live object.
pub fn object_class(id: u64) -> Option<String> {
    registry().lock().ok()?.get(&id).map(|(_, c)| c.clone())
}

// ---------------------------------------------------------------- reflection

/// Call a no-arg method returning a `String` on `obj`, then convert.
fn call_string_method(
    env: &mut JNIEnv,
    obj: &JObject,
    name: &str,
    sig: &str,
) -> Result<String, JErr> {
    match env.call_method(obj, name, sig, &[]) {
        Ok(v) => {
            let o = v
                .l()
                .map_err(|e| JErr::Message(format!("{}.{} result: {}", name, sig, e)))?;
            if o.is_null() {
                return Ok(String::new());
            }
            jstring_to_string(env, &JString::from(o))
        }
        Err(jni::errors::Error::JavaException) => Err(take_pending(env)
            .unwrap_or_else(|| JErr::Message(format!("{} raised", name)))),
        Err(e) => Err(JErr::Message(format!("{} failed: {}", name, e))),
    }
}

fn find_class_obj<'a>(env: &mut JNIEnv<'a>, name: &str) -> Result<JClass<'a>, JErr> {
    env.find_class(slashed(name))
        .map_err(|e| match take_pending(env) {
            Some(JErr::Thrown(t)) => JErr::Message(format!("findClass: Class not found: {}", name)),
            Some(other) => other,
            None => JErr::Message(format!("findClass: Class not found: {} ({})", name, e)),
        })
}

/// Whether the JVM can load `name` (Tier-3 class validation).
pub fn class_exists(name: &str) -> Option<Result<bool, JErr>> {
    with_env(|env| match find_class_obj(env, name) {
        Ok(_) => Ok(true),
        Err(JErr::Message(m)) if m.starts_with("findClass: Class not found") => Ok(false),
        Err(e) => Err(e),
    })
}

/// Canonical name of the class named `name` (`Class.getCanonicalName`).
pub fn class_canonical_name(name: &str) -> Option<Result<String, JErr>> {
    with_env(|env| {
        let clazz = find_class_obj(env, name)?;
        call_string_method(env, &JObject::from(clazz), "getCanonicalName", "()Ljava/lang/String;")
    })
}

/// Resolve `Class[]` elements' canonical names from a `Class` object array.
fn class_array_names(env: &mut JNIEnv, arr: &JObjectArray) -> Result<Vec<String>, JErr> {
    let n = env
        .get_array_length(arr)
        .map_err(|e| JErr::Message(format!("getArrayLength failed: {}", e)))?;
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let el = env
            .get_object_array_element(arr, i)
            .map_err(|e| JErr::Message(format!("array element {} failed: {}", i, e)))?;
        out.push(call_string_method(
            env,
            &el,
            "getCanonicalName",
            "()Ljava/lang/String;",
        )?);
    }
    Ok(out)
}

/// `Modifier.isStatic(int)`.
fn is_static_modifier(env: &mut JNIEnv, modifiers: i32) -> Result<bool, JErr> {
    match env.call_static_method(
        "java/lang/reflect/Modifier",
        "isStatic",
        "(I)Z",
        &[JValue::Int(modifiers)],
    ) {
        Ok(v) => v
            .z()
            .map_err(|e| JErr::Message(format!("isStatic result: {}", e))),
        Err(jni::errors::Error::JavaException) => Err(take_pending(env)
            .unwrap_or_else(|| JErr::Message("Modifier.isStatic raised".into()))),
        Err(e) => Err(JErr::Message(format!("Modifier.isStatic failed: {}", e))),
    }
}

/// Reflection binding for `findMethod(class; name; *argTypes)`.
///
/// Kotlin uses `Class.getMethod(name, *argTypes)`: an exact match on the
/// declared parameter types (and, with no `argTypes`, only the no-arg
/// overload).
pub fn bind_method(
    class: &str,
    name: &str,
    arg_types: &[String],
) -> Option<Result<MethodBind, JErr>> {
    with_env(|env| {
        let clazz = find_class_obj(env, class)?;
        let methods = env
            .call_method(
                &clazz,
                "getMethods",
                "()[Ljava/lang/reflect/Method;",
                &[],
            )
            .map_err(|e| JErr::Message(format!("getMethods failed: {}", e)))?;
        let arr_obj = methods
            .l()
            .map_err(|e| JErr::Message(format!("getMethods result: {}", e)))?;
        let arr = JObjectArray::from(arr_obj);
        let n = env
            .get_array_length(&arr)
            .map_err(|e| JErr::Message(format!("getArrayLength failed: {}", e)))?;
        let mut found: Option<MethodBind> = None;
        for i in 0..n {
            let m = env
                .get_object_array_element(&arr, i)
                .map_err(|e| JErr::Message(format!("method element failed: {}", e)))?;
            let mname = call_string_method(env, &m, "getName", "()Ljava/lang/String;")?;
            if mname != name {
                continue;
            }
            let params_v = env
                .call_method(&m, "getParameterTypes", "()[Ljava/lang/Class;", &[])
                .map_err(|e| JErr::Message(format!("getParameterTypes failed: {}", e)))?;
            let params_obj = params_v
                .l()
                .map_err(|e| JErr::Message(format!("getParameterTypes result: {}", e)))?;
            let params = class_array_names(env, &JObjectArray::from(params_obj))?;
            if params.len() != arg_types.len() {
                continue;
            }
            if !arg_types.is_empty() {
                let want: Vec<String> = arg_types
                    .iter()
                    .map(|t| class_canonical_of_simple(t))
                    .collect();
                if params != want {
                    continue;
                }
            }
            let ret_v = env
                .call_method(&m, "getReturnType", "()Ljava/lang/Class;", &[])
                .map_err(|e| JErr::Message(format!("getReturnType failed: {}", e)))?;
            let ret_obj = ret_v
                .l()
                .map_err(|e| JErr::Message(format!("getReturnType result: {}", e)))?;
            let ret = call_string_method(env, &ret_obj, "getCanonicalName", "()Ljava/lang/String;")?;
            let mods = env
                .call_method(&m, "getModifiers", "()I", &[])
                .map_err(|e| JErr::Message(format!("getModifiers failed: {}", e)))?
                .i()
                .map_err(|e| JErr::Message(format!("getModifiers result: {}", e)))?;
            let is_static = is_static_modifier(env, mods)?;
            found = Some(MethodBind { params, ret, is_static });
            break;
        }
        found.ok_or_else(|| {
            JErr::Message(format!(
                "Method not found: {}{}",
                name,
                if arg_types.is_empty() {
                    String::new()
                } else {
                    format!("({})", arg_types.join(","))
                }
            ))
        })
    })
}

/// Normalise an argument type name for comparison with reflection output:
/// accept both `int` and `java.lang.Integer`-style names as given, but
/// translate JNI's own forms (`I`, `Ljava.lang.String;`) into canonical ones.
fn class_canonical_of_simple(t: &str) -> String {
    match t {
        "I" => "int".to_string(),
        "J" => "long".to_string(),
        "Z" => "boolean".to_string(),
        "C" => "char".to_string(),
        "B" => "byte".to_string(),
        "S" => "short".to_string(),
        "F" => "float".to_string(),
        "D" => "double".to_string(),
        "V" => "void".to_string(),
        other => {
            if let Some(rest) = other.strip_prefix("L") {
                rest.trim_end_matches(';').replace('/', ".")
            } else if let Some(rest) = other.strip_prefix('[') {
                format!("{}[]", class_canonical_of_simple(rest))
            } else {
                other.to_string()
            }
        }
    }
}

/// Reflection binding for `findConstructor(class; *argTypes)`.
pub fn bind_ctor(class: &str, arg_types: &[String]) -> Option<Result<CtorBind, JErr>> {
    with_env(|env| {
        let clazz = find_class_obj(env, class)?;
        let ctors = env
            .call_method(
                &clazz,
                "getConstructors",
                "()[Ljava/lang/reflect/Constructor;",
                &[],
            )
            .map_err(|e| JErr::Message(format!("getConstructors failed: {}", e)))?;
        let arr_obj = ctors
            .l()
            .map_err(|e| JErr::Message(format!("getConstructors result: {}", e)))?;
        let arr = JObjectArray::from(arr_obj);
        let n = env
            .get_array_length(&arr)
            .map_err(|e| JErr::Message(format!("getArrayLength failed: {}", e)))?;
        let want: Vec<String> = arg_types
            .iter()
            .map(|t| class_canonical_of_simple(t))
            .collect();
        for i in 0..n {
            let c = env
                .get_object_array_element(&arr, i)
                .map_err(|e| JErr::Message(format!("ctor element failed: {}", e)))?;
            let params_v = env
                .call_method(&c, "getParameterTypes", "()[Ljava/lang/Class;", &[])
                .map_err(|e| JErr::Message(format!("getParameterTypes failed: {}", e)))?;
            let params_obj = params_v
                .l()
                .map_err(|e| JErr::Message(format!("getParameterTypes result: {}", e)))?;
            let params = class_array_names(env, &JObjectArray::from(params_obj))?;
            if params == want {
                return Ok(CtorBind { params });
            }
        }
        Err(JErr::Message(format!(
            "Constructor not found with args: {}",
            arg_types.join(", ")
        )))
    })
}

/// Reflection binding for `findField(class; name)`.
pub fn bind_field(class: &str, name: &str) -> Option<Result<FieldBind, JErr>> {
    with_env(|env| {
        let clazz = find_class_obj(env, class)?;
        let fname = java_string(env, name)?;
        let fobj = JObject::from(fname);
        let field = match env.call_method(
            &clazz,
            "getField",
            "(Ljava/lang/String;)Ljava/lang/reflect/Field;",
            &[JValue::Object(&fobj)],
        ) {
            Ok(v) => v
                .l()
                .map_err(|e| JErr::Message(format!("getField result: {}", e)))?,
            Err(jni::errors::Error::JavaException) => {
                let taken = take_pending(env);
                if std::env::var_os("KAP_JVM_DEBUG").is_some() {
                    eprintln!("JVM DEBUG: findField {}.{} -> {:?}", class, name, taken);
                }
                return Err(taken.unwrap_or_else(|| {
                    JErr::Message(format!("Field not found: {}", name))
                }));
            }
            Err(e) => return Err(JErr::Message(format!("getField failed: {}", e))),
        };
        let t_v = env
            .call_method(&field, "getType", "()Ljava/lang/Class;", &[])
            .map_err(|e| JErr::Message(format!("getType failed: {}", e)))?;
        let t_obj = t_v
            .l()
            .map_err(|e| JErr::Message(format!("getType result: {}", e)))?;
        let ftype = call_string_method(env, &t_obj, "getCanonicalName", "()Ljava/lang/String;")?;
        let mods = env
            .call_method(&field, "getModifiers", "()I", &[])
            .map_err(|e| JErr::Message(format!("getModifiers failed: {}", e)))?
            .i()
            .map_err(|e| JErr::Message(format!("getModifiers result: {}", e)))?;
        let is_static = is_static_modifier(env, mods)?;
        Ok(FieldBind { ftype, is_static })
    })
}

// ---------------------------------------------------------------- invocation

/// Build the `JValue` argument list for a call, per the target parameter types.
///
/// Kotlin converts each Kap argument with `toJava(value, paramType)`; the same
/// rules are applied here (numeric widening/truncation within range, strings
/// and live objects by reference, `null` as `null`).
fn build_args<'local, 'obj>(
    env: &mut JNIEnv<'local>,
    params: &[String],
    args: &[JArg],
    strings: &'obj mut Vec<JString<'local>>,
    objs: &'obj mut Vec<JObject<'local>>,
    null_obj: &'obj JObject<'local>,
) -> Result<Vec<JValue<'local, 'obj>>, JErr> {
    if params.len() != args.len() {
        return Err(JErr::Message(format!(
            "Expected {} arguments, got {}",
            params.len(),
            args.len()
        )));
    }
    // Two phases: first create every owned local reference (so no `JValue`
    // borrows a Vec that is still growing), then build the borrowed list.
    enum Plan {
        Null,
        Bool(bool),
        Byte(i8),
        Short(i16),
        Int(i32),
        Long(i64),
        Float(f32),
        Double(f64),
        Char(u16),
        Str(usize),
        Obj(usize),
    }
    let mut plan: Vec<Plan> = Vec::with_capacity(args.len());
    for (p, a) in params.iter().zip(args.iter()) {
        let desc = descriptor_of(p)?;
        plan.push(match (desc.as_str(), a) {
            (_, JArg::Null) => Plan::Null,
            ("Z", JArg::Bool(b)) => Plan::Bool(*b),
            ("B", v) => Plan::Byte(num_i64(v)? as i8),
            ("S", v) => Plan::Short(num_i64(v)? as i16),
            ("I", v) => Plan::Int(num_i64(v)? as i32),
            ("J", v) => Plan::Long(num_i64(v)?),
            ("F", v) => Plan::Float(num_f64(v)? as f32),
            ("D", v) => Plan::Double(num_f64(v)?),
            ("C", JArg::Char(c)) => Plan::Char(*c as u16),
            ("C", v) => Plan::Char(num_i64(v)? as u16),
            ("Ljava/lang/String;", JArg::Str(s)) => {
                let js = java_string(env, s)?;
                strings.push(js);
                Plan::Str(strings.len() - 1)
            }
            (_, JArg::Obj(id)) => {
                let lr = local_ref(env, *id)?;
                objs.push(lr);
                Plan::Obj(objs.len() - 1)
            }
            ("[B", JArg::Bytes(b)) => {
                let arr = env
                    .new_byte_array(b.len() as i32)
                    .map_err(|e| JErr::Message(format!("newByteArray failed: {}", e)))?;
                let signed: Vec<i8> = b.iter().map(|x| *x as i8).collect();
                env.set_byte_array_region(&arr, 0, &signed)
                    .map_err(|e| JErr::Message(format!("setByteArrayRegion failed: {}", e)))?;
                objs.push(JObject::from(arr));
                Plan::Obj(objs.len() - 1)
            }
            ("[B", JArg::Longs(v)) => {
                // Kotlin's `toJava` builds the Java array type the parameter
                // asks for; `String.getBytes`' `byte[]` result is handed
                // straight back into a `byte[]` constructor slot.
                let signed: Vec<i8> = v.iter().map(|x| *x as i8).collect();
                let arr = env
                    .new_byte_array(signed.len() as i32)
                    .map_err(|e| JErr::Message(format!("newByteArray failed: {}", e)))?;
                env.set_byte_array_region(&arr, 0, &signed)
                    .map_err(|e| JErr::Message(format!("setByteArrayRegion failed: {}", e)))?;
                objs.push(JObject::from(arr));
                Plan::Obj(objs.len() - 1)
            }
            ("[I", JArg::Longs(v)) => {
                let vals: Vec<i32> = v.iter().map(|x| *x as i32).collect();
                let arr = env
                    .new_int_array(vals.len() as i32)
                    .map_err(|e| JErr::Message(format!("newIntArray failed: {}", e)))?;
                env.set_int_array_region(&arr, 0, &vals)
                    .map_err(|e| JErr::Message(format!("setIntArrayRegion failed: {}", e)))?;
                objs.push(JObject::from(arr));
                Plan::Obj(objs.len() - 1)
            }
            ("[J", JArg::Longs(v)) => {
                let arr = env
                    .new_long_array(v.len() as i32)
                    .map_err(|e| JErr::Message(format!("newLongArray failed: {}", e)))?;
                env.set_long_array_region(&arr, 0, v)
                    .map_err(|e| JErr::Message(format!("setLongArrayRegion failed: {}", e)))?;
                objs.push(JObject::from(arr));
                Plan::Obj(objs.len() - 1)
            }
            ("[S", JArg::Longs(v)) => {
                let vals: Vec<i16> = v.iter().map(|x| *x as i16).collect();
                let arr = env
                    .new_short_array(vals.len() as i32)
                    .map_err(|e| JErr::Message(format!("newShortArray failed: {}", e)))?;
                env.set_short_array_region(&arr, 0, &vals)
                    .map_err(|e| JErr::Message(format!("setShortArrayRegion failed: {}", e)))?;
                objs.push(JObject::from(arr));
                Plan::Obj(objs.len() - 1)
            }
            ("[D", JArg::Longs(v)) => {
                let vals: Vec<f64> = v.iter().map(|x| *x as f64).collect();
                let arr = env
                    .new_double_array(vals.len() as i32)
                    .map_err(|e| JErr::Message(format!("newDoubleArray failed: {}", e)))?;
                env.set_double_array_region(&arr, 0, &vals)
                    .map_err(|e| JErr::Message(format!("setDoubleArrayRegion failed: {}", e)))?;
                objs.push(JObject::from(arr));
                Plan::Obj(objs.len() - 1)
            }
            (d, v) => {
                return Err(JErr::Message(format!(
                    "Cannot convert {:?} to Java type {}",
                    v, d
                )))
            }
        });
    }
    let mut out: Vec<JValue<'local, 'obj>> = Vec::with_capacity(plan.len());
    for p in plan {
        out.push(match p {
            Plan::Null => JValue::Object(null_obj),
            Plan::Bool(b) => JValue::Bool(b as u8),
            Plan::Byte(b) => JValue::Byte(b),
            Plan::Short(s) => JValue::Short(s),
            Plan::Int(i) => JValue::Int(i),
            Plan::Long(l) => JValue::Long(l),
            Plan::Float(f) => JValue::Float(f),
            Plan::Double(d) => JValue::Double(d),
            Plan::Char(c) => JValue::Char(c),
            Plan::Str(i) => JValue::Object(strings[i].as_ref()),
            Plan::Obj(i) => JValue::Object(&objs[i]),
        });
    }
    Ok(out)
}

fn num_i64(v: &JArg) -> Result<i64, JErr> {
    match v {
        JArg::Byte(n) => Ok(*n as i64),
        JArg::Short(n) => Ok(*n as i64),
        JArg::Int(n) => Ok(*n as i64),
        JArg::Long(n) => Ok(*n),
        JArg::Float(f) => Ok(*f as i64),
        JArg::Double(d) => Ok(*d as i64),
        JArg::Char(c) => Ok(*c as i64),
        JArg::Bool(b) => Ok(if *b { 1 } else { 0 }),
        other => Err(JErr::Message(format!("not a number: {:?}", other))),
    }
}

fn num_f64(v: &JArg) -> Result<f64, JErr> {
    match v {
        JArg::Byte(n) => Ok(*n as f64),
        JArg::Short(n) => Ok(*n as f64),
        JArg::Int(n) => Ok(*n as f64),
        JArg::Long(n) => Ok(*n as f64),
        JArg::Float(f) => Ok(*f as f64),
        JArg::Double(d) => Ok(*d),
        other => Err(JErr::Message(format!("not a number: {:?}", other))),
    }
}

/// Convert a returned `JValue` into a [`JVal`], registering objects.
fn convert_return(
    env: &mut JNIEnv,
    v: jni::objects::JValueOwned,
    ret: &str,
    keep_objects: bool,
) -> Result<JVal, JErr> {
    let desc = descriptor_of(ret)?;
    Ok(match desc.as_str() {
        "V" => JVal::Void,
        "Z" => JVal::Bool(v.z().map_err(|e| JErr::Message(format!("bool: {}", e)))?),
        "B" => JVal::Byte(v.b().map_err(|e| JErr::Message(format!("byte: {}", e)))?),
        "S" => JVal::Short(v.s().map_err(|e| JErr::Message(format!("short: {}", e)))?),
        "I" => JVal::Int(v.i().map_err(|e| JErr::Message(format!("int: {}", e)))?),
        "J" => JVal::Long(v.j().map_err(|e| JErr::Message(format!("long: {}", e)))?),
        "F" => JVal::Float(v.f().map_err(|e| JErr::Message(format!("float: {}", e)))?),
        "D" => JVal::Double(v.d().map_err(|e| JErr::Message(format!("double: {}", e)))?),
        "C" => JVal::Char(
            char::from_u32(v.c().map_err(|e| JErr::Message(format!("char: {}", e)))? as u32)
                .unwrap_or('\u{fffd}'),
        ),
        "[B" => {
            let o = v.l().map_err(|e| JErr::Message(format!("byte[]: {}", e)))?;
            let arr = jni::objects::JByteArray::from(o);
            let n = env
                .get_array_length(&arr)
                .map_err(|e| JErr::Message(format!("byte[] length: {}", e)))?;
            let mut buf = vec![0i8; n as usize];
            env.get_byte_array_region(&arr, 0, &mut buf)
                .map_err(|e| JErr::Message(format!("byte[] region: {}", e)))?;
            JVal::Bytes(buf.into_iter().map(|x| x as u8).collect())
        }
        "Ljava/lang/String;" => {
            let o = v.l().map_err(|e| JErr::Message(format!("string: {}", e)))?;
            if o.is_null() {
                JVal::Null
            } else {
                JVal::Str(jstring_to_string(env, &JString::from(o))?)
            }
        }
        _ => {
            let o = v.l().map_err(|e| JErr::Message(format!("object: {}", e)))?;
            if o.is_null() {
                JVal::Null
            } else {
                // `javaObjToKap` only maps a known set of Java types; anything
                // else is Kotlin's "Unexpected JVM type: …".
                let class = class_name_of(env, &o)?;
                match class.as_str() {
                    "java.lang.Integer" => JVal::Int(
                        int_of_boxed(env, &o, "intValue", "()I")? as i32,
                    ),
                    "java.lang.Long" => JVal::Long(int_of_boxed(env, &o, "longValue", "()J")?),
                    "java.lang.Short" => {
                        JVal::Short(int_of_boxed(env, &o, "shortValue", "()S")? as i16)
                    }
                    "java.lang.Byte" => {
                        JVal::Byte(int_of_boxed(env, &o, "byteValue", "()B")? as i8)
                    }
                    "java.lang.Boolean" => JVal::Bool(boxed_bool(env, &o)?),
                    "java.lang.Double" => JVal::Double(boxed_double(env, &o, "doubleValue")?),
                    "java.lang.Float" => {
                        JVal::Float(boxed_double(env, &o, "floatValue")? as f32)
                    }
                    "java.lang.Character" => JVal::Char(
                        char::from_u32(boxed_int(env, &o, "charValue", "()C")? as u32)
                            .unwrap_or('\u{fffd}'),
                    ),
                    "java.math.BigInteger" | "java.math.BigDecimal" => {
                        let s = call_string_method(env, &o, "toString", "()Ljava/lang/String;")?;
                        JVal::Str(s)
                    }
                    // Unmapped class. `getField` hands back the RAW JVM value
                    // like Kotlin's `JvmInstanceValue(field.get(obj))` — e.g.
                    // `XPathConstants.STRING` is a `javax.xml.namespace.QName` —
                    // so register it and carry the handle; `fromJvm` is what
                    // raises "Unexpected JVM type: …" (javaObjToKap).
                    _ => {
                        if keep_objects {
                            JVal::Obj {
                                id: register(env, &o)?,
                                class,
                            }
                        } else {
                            JVal::Other { class }
                        }
                    }
                }
            }
        }
    })
}

fn int_of_boxed(env: &mut JNIEnv, o: &JObject, name: &str, sig: &str) -> Result<i64, JErr> {
    match env.call_method(o, name, sig, &[]) {
        Ok(v) => match sig {
            "()J" => v.j().map_err(|e| JErr::Message(format!("{}: {}", name, e))),
            "()S" => v.s().map(|x| x as i64).map_err(|e| JErr::Message(format!("{}: {}", name, e))),
            "()B" => v.b().map(|x| x as i64).map_err(|e| JErr::Message(format!("{}: {}", name, e))),
            _ => v.i().map(|x| x as i64).map_err(|e| JErr::Message(format!("{}: {}", name, e))),
        },
        Err(jni::errors::Error::JavaException) => Err(take_pending(env)
            .unwrap_or_else(|| JErr::Message(format!("{} raised", name)))),
        Err(e) => Err(JErr::Message(format!("{} failed: {}", name, e))),
    }
}

fn boxed_int(env: &mut JNIEnv, o: &JObject, name: &str, sig: &str) -> Result<u16, JErr> {
    match env.call_method(o, name, sig, &[]) {
        Ok(v) => v.c().map_err(|e| JErr::Message(format!("{}: {}", name, e))),
        Err(jni::errors::Error::JavaException) => Err(take_pending(env)
            .unwrap_or_else(|| JErr::Message(format!("{} raised", name)))),
        Err(e) => Err(JErr::Message(format!("{} failed: {}", name, e))),
    }
}

fn boxed_double(env: &mut JNIEnv, o: &JObject, name: &str) -> Result<f64, JErr> {
    match env.call_method(o, name, "()D", &[]) {
        Ok(v) => v.d().map_err(|e| JErr::Message(format!("{}: {}", name, e))),
        Err(jni::errors::Error::JavaException) => Err(take_pending(env)
            .unwrap_or_else(|| JErr::Message(format!("{} raised", name)))),
        Err(e) => Err(JErr::Message(format!("{} failed: {}", name, e))),
    }
}

fn boxed_bool(env: &mut JNIEnv, o: &JObject) -> Result<bool, JErr> {
    match env.call_method(o, "booleanValue", "()Z", &[]) {
        Ok(v) => v.z().map_err(|e| JErr::Message(format!("booleanValue: {}", e))),
        Err(jni::errors::Error::JavaException) => Err(take_pending(env)
            .unwrap_or_else(|| JErr::Message("booleanValue raised".into()))),
        Err(e) => Err(JErr::Message(format!("booleanValue failed: {}", e))),
    }
}

/// Invoke a bound method (`callMethod`).
///
/// `instance` is `None` for Kotlin's `null` instance — legal only for static
/// methods (otherwise Kotlin's `NullPointerException` path: "Called method …
/// with a null instance").
pub fn call_method(
    class: &str,
    name: &str,
    params: &[String],
    ret: &str,
    is_static: bool,
    instance: Option<u64>,
    args: &[JArg],
) -> Option<Result<JVal, JErr>> {
    with_env(|env| {
        let sig = {
            let mut s = String::from("(");
            for p in params {
                s.push_str(&descriptor_of(p)?);
            }
            s.push(')');
            s.push_str(&descriptor_of(ret)?);
            s
        };
        let mut strings: Vec<JString> = Vec::new();
        let mut objs: Vec<JObject> = Vec::new();
        let null_obj = JObject::null();
        let vals = build_args(env, params, args, &mut strings, &mut objs, &null_obj)?;
        let class_slashed = slashed(class);
        let result = if is_static {
            env.call_static_method(class_slashed.as_str(), name, sig.as_str(), &vals)
        } else {
            match instance {
                None => {
                    return Err(JErr::Message(format!(
                        "Called method {}.{} with a null instance",
                        class, name
                    )))
                }
                Some(id) => {
                    let target = local_ref(env, id)?;
                    env.call_method(&target, name, sig.as_str(), &vals)
                }
            }
        };
        match result {
            Ok(v) => convert_return(env, v, ret, true),
            Err(jni::errors::Error::JavaException) => Err(take_pending(env)
                .unwrap_or_else(|| JErr::Message(format!("{}.{} raised", class, name)))),
            Err(e) => Err(JErr::Message(format!(
                "Error when calling {}.{}: {}",
                class, name, e
            ))),
        }
    })
}

/// Construct an instance (`createInstance`).
pub fn create_instance(
    class: &str,
    params: &[String],
    args: &[JArg],
) -> Option<Result<JVal, JErr>> {
    with_env(|env| {
        let sig = {
            let mut s = String::from("(");
            for p in params {
                s.push_str(&descriptor_of(p)?);
            }
            s.push_str(")V");
            s
        };
        let mut strings: Vec<JString> = Vec::new();
        let mut objs: Vec<JObject> = Vec::new();
        let null_obj = JObject::null();
        let vals = build_args(env, params, args, &mut strings, &mut objs, &null_obj)?;
        match env.new_object(slashed(class).as_str(), sig.as_str(), &vals) {
            Ok(obj) => {
                if obj.is_null() {
                    return Ok(JVal::Null);
                }
                let id = register(env, &obj)?;
                Ok(JVal::Obj {
                    id,
                    class: class.to_string(),
                })
            }
            Err(jni::errors::Error::JavaException) => Err(take_pending(env)
                .unwrap_or_else(|| JErr::Message(format!("{} constructor raised", class)))),
            Err(e) => Err(JErr::Message(format!(
                "Error constructing {}: {}",
                class, e
            ))),
        }
    })
}

/// Read a field (`getField`). `instance` is `None` for `null` (static fields
/// are read from the class).
pub fn get_field(
    class: &str,
    name: &str,
    ftype: &str,
    is_static: bool,
    instance: Option<u64>,
) -> Option<Result<JVal, JErr>> {
    with_env(|env| {
        let desc = descriptor_of(ftype)?;
        let class_slashed = slashed(class);
        let result = if is_static {
            env.get_static_field(class_slashed.as_str(), name, desc.as_str())
        } else {
            match instance {
                None => {
                    return Err(JErr::Message(format!(
                        "Called field {}.{} with a null instance",
                        class, name
                    )))
                }
                Some(id) => {
                    let target = local_ref(env, id)?;
                    env.get_field(&target, name, desc.as_str())
                }
            }
        };
        match result {
            Ok(v) => convert_return(env, v, ftype, true),
            Err(jni::errors::Error::JavaException) => Err(take_pending(env)
                .unwrap_or_else(|| JErr::Message(format!("{}.{} raised", class, name)))),
            Err(e) => Err(JErr::Message(format!(
                "Error reading field {}.{}: {}",
                class, name, e
            ))),
        }
    })
}

/// Materialise a `java.lang.String` object and return `(id, class)`.
///
/// Kotlin's `toJvmString`/`toJvmInt`/… return `JvmInstanceValue(boxed)`, i.e. a
/// real JVM object, so code like `xml.kap`'s `readString` can pass
/// `jvm:toJvmString s` as the RECEIVER of `String.getBytes` — and
/// `jvm:callMethod` resolves the receiver via `toJava(args[1],
/// method.declaringClass)`.
pub fn new_string_id(s: &str) -> Option<Result<(u64, String), JErr>> {
    with_env(|env| {
        let o = env
            .new_string(s)
            .map_err(|e| JErr::Message(format!("newString failed: {}", e)))?;
        let obj = JObject::from(o);
        let id = register(env, &obj)?;
        Ok((id, "java.lang.String".to_string()))
    })
}

/// `Object.toString()` of a live object (`toStringCalledWhenPrinting`).
pub fn object_to_string(id: u64) -> Option<Result<String, JErr>> {
    with_env(|env| {
        let target = local_ref(env, id)?;
        call_string_method(env, &target, "toString", "()Ljava/lang/String;")
    })
}

/// `Class.isInstance(obj)` for a live object against a named class.
pub fn instance_of(id: u64, class: &str) -> Option<Result<bool, JErr>> {
    with_env(|env| {
        let clazz = find_class_obj(env, class)?;
        let target = local_ref(env, id)?;
        let r = match env.call_method(
            &clazz,
            "isInstance",
            "(Ljava/lang/Object;)Z",
            &[JValue::Object(&target)],
        ) {
            Ok(v) => v.z().map_err(|e| JErr::Message(format!("isInstance: {}", e)))?,
            Err(jni::errors::Error::JavaException) => {
                return Err(take_pending(env)
                    .unwrap_or_else(|| JErr::Message("isInstance raised".into())))
            }
            Err(e) => return Err(JErr::Message(format!("isInstance failed: {}", e))),
        };
        Ok(r)
    })
}
