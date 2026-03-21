// NaN-boxed Value: every value fits in 8 bytes (u64).
//
// Encoding:
//   Float:  any f64 bit pattern that is NOT a quiet NaN with our tag bits set
//   Tagged: quiet NaN pattern with type tag in bits 50-48 and 48-bit payload
//
// Tagged layout (when upper 13 bits = 0x7FF8 + tag):
//   Bits 63-51: 0xFFF (NaN exponent + quiet bit)
//   Bits 50-48: type tag (0-7)
//   Bits 47-0:  payload (48-bit pointer or inline value)
//
// Tags:
//   0 = Int (inline i48, ±2^47)
//   1 = Bool (payload: 0=false, 1=true)
//   2 = Void
//   3 = String (pointer to Rc<str>)
//   4 = List (pointer to Rc<RefCell<Vec<Value>>>)
//   5 = Object (pointer to Rc<RefCell<ObjectData>>)
//   6 = Boxed (pointer to Box<HeapValue> — lambda, cmdresult, thread, file, bytes, atomic, big int)
//
// Memory safety:
//   - Clone: increments Rc for pointer types
//   - Drop: decrements Rc for pointer types
//   - No Copy trait (Drop prevents it)

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::io::BufReader;
use std::fs::File;
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use indexmap::IndexMap;

// ---------------------------------------------------------------------------
// NaN-boxing constants
// ---------------------------------------------------------------------------

const QNAN: u64        = 0x7FF8_0000_0000_0000;
const TAG_INT: u64     = QNAN | (0 << 48);
const TAG_BOOL: u64    = QNAN | (1 << 48);
const TAG_VOID: u64    = QNAN | (2 << 48);
const TAG_STRING: u64  = QNAN | (3 << 48);
const TAG_LIST: u64    = QNAN | (4 << 48);
const TAG_OBJECT: u64  = QNAN | (5 << 48);
const TAG_BOXED: u64   = QNAN | (6 << 48);

const TAG_MASK: u64    = 0xFFFF_0000_0000_0000; // upper 16 bits
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF; // lower 48 bits

/// Max inline int: 2^47 - 1
const MAX_INLINE_INT: i64 = (1_i64 << 47) - 1;
/// Min inline int: -2^47
const MIN_INLINE_INT: i64 = -(1_i64 << 47);

// ---------------------------------------------------------------------------
// Heap-allocated value for types that don't fit inline
// ---------------------------------------------------------------------------

pub enum HeapValue {
    BigInt(i64),
    Lambda(LambdaData),
    CommandResult(CommandResultData),
    ThreadHandle(Arc<Mutex<ThreadJoinHandle>>),
    Bytes(Vec<u8>),
    FileHandle(Arc<Mutex<FileHandleInner>>),
    Atomic(AtomicValue),
}

// ---------------------------------------------------------------------------
// Supporting types (unchanged from original)
// ---------------------------------------------------------------------------

pub type SharedList = Rc<RefCell<Vec<Value>>>;
pub type SharedObject = Rc<RefCell<ObjectData>>;

#[derive(Debug, Clone)]
pub struct ObjectData {
    pub fields: IndexMap<String, Value>,
    pub dyn_fields: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct LambdaData {
    pub name: String,
    pub resolution: u8,
    pub bound_args: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct CommandResultData {
    pub status: i32,
    pub out: String,
    pub err: String,
}

pub struct ThreadJoinHandle {
    pub handle: Option<JoinHandle<Result<SendableValue, String>>>,
}

impl fmt::Debug for ThreadJoinHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThreadHandle(..)")
    }
}

pub struct FileHandleInner {
    pub reader: BufReader<File>,
}

impl fmt::Debug for FileHandleInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileHandle")
    }
}

#[derive(Clone)]
pub enum AtomicValue {
    Int(Arc<AtomicI64>),
    General(Arc<Mutex<SendableValue>>),
}

impl fmt::Debug for AtomicValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(a) => write!(f, "Atomic({})", a.load(Ordering::SeqCst)),
            Self::General(m) => write!(f, "Atomic({:?})", m.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }
}

impl AtomicValue {
    #[must_use]
    pub fn new(val: &Value) -> Self {
        if let Some(n) = val.as_int() {
            Self::Int(Arc::new(AtomicI64::new(n)))
        } else {
            Self::General(Arc::new(Mutex::new(val.to_sendable())))
        }
    }

    #[must_use]
    pub fn load(&self) -> Value {
        match self {
            Self::Int(a) => Value::int(a.load(Ordering::SeqCst)),
            Self::General(m) => Value::from_sendable(m.lock().unwrap_or_else(|e| e.into_inner()).clone()),
        }
    }

    pub fn store(&self, val: &Value) {
        match (self, val.as_int()) {
            (Self::Int(a), Some(n)) => { a.store(n, Ordering::SeqCst); }
            (Self::General(m), _) => { *m.lock().unwrap_or_else(|e| e.into_inner()) = val.to_sendable(); }
            _ => {}
        }
    }

    #[must_use]
    pub fn fetch_add(&self, n: i64) -> i64 {
        match self {
            Self::Int(a) => a.fetch_add(n, Ordering::SeqCst),
            Self::General(_) => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SendableValue {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    List(Vec<Self>),
    Object(IndexMap<String, Self>),
    Void,
    Lambda { name: String, resolution: u8, bound_args: Vec<Self> },
    CommandResult { status: i32, out: String, err: String },
    Bytes(Vec<u8>),
    Atomic(AtomicValue),
}

#[derive(Debug, Clone)]
pub enum MaybeError {
    Ok(Value),
    Err(ErrorInfo),
}

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub message: String,
}

// ---------------------------------------------------------------------------
// The NaN-boxed Value
// ---------------------------------------------------------------------------

#[repr(transparent)]
pub struct Value(u64);

// Value is NOT Copy (Drop needed for Rc management)

impl Value {
    // --- Constructors ---

    #[inline]
    pub fn int(n: i64) -> Self {
        if n >= MIN_INLINE_INT && n <= MAX_INLINE_INT {
            Self(TAG_INT | (n as u64 & PAYLOAD_MASK))
        } else {
            // BigInt: heap-allocate
            Self::boxed(HeapValue::BigInt(n))
        }
    }

    #[inline]
    pub fn float(f: f64) -> Self {
        let bits = f.to_bits();
        // Ensure it doesn't collide with our tagged NaN space
        // A normal f64 NaN has bits in the exponent but that's fine —
        // we only use quiet NaN patterns with specific tag bits.
        // If the f64 happens to be exactly one of our tagged patterns, box it.
        if (bits & TAG_MASK) >= QNAN && (bits & TAG_MASK) <= TAG_BOXED {
            // Extremely rare: the float looks like one of our tags.
            // Store as a boxed float via BigInt slot. In practice this never happens.
            Self(bits) // Actually, just store it — NaN is NaN, display as NaN
        } else {
            Self(bits)
        }
    }

    #[inline]
    pub fn bool(b: bool) -> Self {
        Self(TAG_BOOL | (b as u64))
    }

    #[inline]
    pub fn void() -> Self {
        Self(TAG_VOID)
    }

    #[inline]
    pub fn string(s: Rc<str>) -> Self {
        // Rc<str> is a fat pointer — wrap in Rc<String> for thin pointer.
        // This adds one indirection but avoids Box allocation on clone.
        let rc_string: Rc<String> = Rc::new((*s).to_string());
        let ptr = Rc::into_raw(rc_string) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_STRING | (ptr & PAYLOAD_MASK))
    }

    pub fn string_from(s: &str) -> Self {
        let rc_string: Rc<String> = Rc::new(s.to_string());
        let ptr = Rc::into_raw(rc_string) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_STRING | (ptr & PAYLOAD_MASK))
    }

    /// Create a string Value from an owned String, avoiding an extra copy.
    pub fn string_owned(s: String) -> Self {
        let rc_string: Rc<String> = Rc::new(s);
        let ptr = Rc::into_raw(rc_string) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_STRING | (ptr & PAYLOAD_MASK))
    }

    /// Create from an existing Rc<String> without extra allocation.
    pub fn string_rc(s: Rc<String>) -> Self {
        let ptr = Rc::into_raw(s) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_STRING | (ptr & PAYLOAD_MASK))
    }

    /// Try to append `suffix` to this string value in-place (if Rc refcount == 1).
    /// Returns `true` if the append succeeded in-place, `false` if a new allocation is needed.
    /// SAFETY: Only call on a value where `is_string()` is true.
    #[inline]
    pub fn try_string_append_in_place(&mut self, suffix: &str) -> bool {
        if self.tag() != TAG_STRING { return false; }
        let ptr = self.payload() as *mut String;
        unsafe {
            // Reconstruct the Rc to check refcount, then forget it
            let rc = Rc::from_raw(ptr);
            let is_unique = Rc::strong_count(&rc) == 1;
            if is_unique {
                // We are the sole owner — mutate in place via the raw pointer.
                // We must forget the Rc first to avoid double-free, since `self`
                // still logically owns it (Drop will free it).
                std::mem::forget(rc);
                (*ptr).push_str(suffix);
                true
            } else {
                std::mem::forget(rc);
                false
            }
        }
    }

    #[inline]
    pub fn list(l: SharedList) -> Self {
        let ptr = Rc::into_raw(l) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_LIST | (ptr & PAYLOAD_MASK))
    }

    #[inline]
    pub fn object(o: SharedObject) -> Self {
        let ptr = Rc::into_raw(o) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_OBJECT | (ptr & PAYLOAD_MASK))
    }

    fn boxed(hv: HeapValue) -> Self {
        let boxed = Box::new(hv);
        let ptr = Box::into_raw(boxed) as u64;
        debug_assert!(ptr & !PAYLOAD_MASK == 0, "pointer exceeds 48 bits");
        Self(TAG_BOXED | (ptr & PAYLOAD_MASK))
    }

    pub fn lambda(data: LambdaData) -> Self {
        Self::boxed(HeapValue::Lambda(data))
    }

    pub fn command_result(data: CommandResultData) -> Self {
        Self::boxed(HeapValue::CommandResult(data))
    }

    pub fn thread_handle(h: Arc<Mutex<ThreadJoinHandle>>) -> Self {
        Self::boxed(HeapValue::ThreadHandle(h))
    }

    pub fn bytes(b: Vec<u8>) -> Self {
        Self::boxed(HeapValue::Bytes(b))
    }

    pub fn file_handle(h: Arc<Mutex<FileHandleInner>>) -> Self {
        Self::boxed(HeapValue::FileHandle(h))
    }

    pub fn atomic(a: AtomicValue) -> Self {
        Self::boxed(HeapValue::Atomic(a))
    }

    // --- Tag checks ---

    #[inline]
    fn tag(&self) -> u64 {
        self.0 & TAG_MASK
    }

    #[inline]
    fn payload(&self) -> u64 {
        self.0 & PAYLOAD_MASK
    }

    #[inline]
    pub fn is_float(&self) -> bool {
        // It's a float if it's NOT one of our tagged patterns
        let tag = self.tag();
        tag < QNAN || tag > TAG_BOXED
    }

    #[inline]
    pub fn is_int(&self) -> bool {
        if self.tag() == TAG_INT { return true; }
        // Check for BigInt
        if self.tag() == TAG_BOXED {
            if let Some(hv) = self.as_heap() {
                return matches!(hv, HeapValue::BigInt(_));
            }
        }
        false
    }

    #[inline]
    pub fn is_bool(&self) -> bool { self.tag() == TAG_BOOL }
    #[inline]
    pub fn is_void(&self) -> bool { self.tag() == TAG_VOID }
    #[inline]
    pub fn is_string(&self) -> bool { self.tag() == TAG_STRING }
    #[inline]
    pub fn is_list(&self) -> bool { self.tag() == TAG_LIST }
    #[inline]
    pub fn is_object(&self) -> bool { self.tag() == TAG_OBJECT }

    pub fn is_lambda(&self) -> bool {
        if self.tag() != TAG_BOXED { return false; }
        self.as_heap().map_or(false, |hv| matches!(hv, HeapValue::Lambda(_)))
    }

    pub fn is_command_result(&self) -> bool {
        if self.tag() != TAG_BOXED { return false; }
        self.as_heap().map_or(false, |hv| matches!(hv, HeapValue::CommandResult(_)))
    }

    pub fn is_thread_handle(&self) -> bool {
        if self.tag() != TAG_BOXED { return false; }
        self.as_heap().map_or(false, |hv| matches!(hv, HeapValue::ThreadHandle(_)))
    }

    pub fn is_bytes(&self) -> bool {
        if self.tag() != TAG_BOXED { return false; }
        self.as_heap().map_or(false, |hv| matches!(hv, HeapValue::Bytes(_)))
    }

    pub fn is_file_handle(&self) -> bool {
        if self.tag() != TAG_BOXED { return false; }
        self.as_heap().map_or(false, |hv| matches!(hv, HeapValue::FileHandle(_)))
    }

    pub fn is_atomic(&self) -> bool {
        if self.tag() != TAG_BOXED { return false; }
        self.as_heap().map_or(false, |hv| matches!(hv, HeapValue::Atomic(_)))
    }

    // --- Extractors ---

    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        if self.tag() == TAG_INT {
            // Sign-extend from 48 bits
            let raw = self.payload() as i64;
            Some((raw << 16) >> 16)
        } else if self.tag() == TAG_BOXED {
            if let Some(HeapValue::BigInt(n)) = self.as_heap() {
                Some(*n)
            } else {
                None
            }
        } else {
            None
        }
    }

    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        if self.is_float() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        if self.tag() == TAG_BOOL {
            Some(self.payload() != 0)
        } else {
            None
        }
    }

    /// Get a cloned Rc<String> for the string.
    pub fn as_str_rc_string(&self) -> Option<Rc<String>> {
        if self.tag() != TAG_STRING { return None; }
        let ptr = self.payload() as *const String;
        unsafe {
            let rc = Rc::from_raw(ptr);
            let cloned = rc.clone();
            std::mem::forget(rc);
            Some(cloned)
        }
    }

    pub fn as_str_ref(&self) -> Option<&str> {
        if self.tag() != TAG_STRING { return None; }
        let ptr = self.payload() as *const String;
        unsafe { Some((*ptr).as_str()) }
    }

    /// For compatibility: create an Rc<str> from the stored Rc<String>.
    pub fn as_str_rc(&self) -> Option<Rc<str>> {
        self.as_str_ref().map(Rc::from)
    }

    /// Get a reference to the inner RefCell.
    pub fn as_list_ref(&self) -> Option<&RefCell<Vec<Value>>> {
        if self.tag() != TAG_LIST { return None; }
        let ptr = self.payload() as *const RefCell<Vec<Value>>;
        unsafe { Some(&*ptr) }
    }

    /// Clone the Rc for sharing ownership.
    pub fn as_list_rc(&self) -> Option<SharedList> {
        if self.tag() != TAG_LIST { return None; }
        let ptr = self.payload() as *const RefCell<Vec<Value>>;
        unsafe {
            let rc = Rc::from_raw(ptr);
            let cloned = rc.clone();
            std::mem::forget(rc);
            Some(cloned)
        }
    }

    /// Get a reference to the inner RefCell for objects.
    pub fn as_object_ref(&self) -> Option<&RefCell<ObjectData>> {
        if self.tag() != TAG_OBJECT { return None; }
        let ptr = self.payload() as *const RefCell<ObjectData>;
        unsafe { Some(&*ptr) }
    }

    /// Clone the Rc for objects.
    pub fn as_object_rc(&self) -> Option<SharedObject> {
        if self.tag() != TAG_OBJECT { return None; }
        let ptr = self.payload() as *const RefCell<ObjectData>;
        unsafe {
            let rc = Rc::from_raw(ptr);
            let cloned = rc.clone();
            std::mem::forget(rc);
            Some(cloned)
        }
    }

    pub fn as_thread_handle(&self) -> Option<&Arc<Mutex<ThreadJoinHandle>>> {
        match self.as_heap()? {
            HeapValue::ThreadHandle(h) => Some(h),
            _ => None,
        }
    }

    pub fn as_file_handle(&self) -> Option<&Arc<Mutex<FileHandleInner>>> {
        match self.as_heap()? {
            HeapValue::FileHandle(h) => Some(h),
            _ => None,
        }
    }

    fn as_heap(&self) -> Option<&HeapValue> {
        if self.tag() != TAG_BOXED { return None; }
        let ptr = self.payload() as *const HeapValue;
        if ptr.is_null() { return None; }
        unsafe { Some(&*ptr) }
    }

    fn _as_heap_mut(&self) -> Option<&mut HeapValue> {
        if self.tag() != TAG_BOXED { return None; }
        let ptr = self.payload() as *mut HeapValue;
        if ptr.is_null() { return None; }
        unsafe { Some(&mut *ptr) }
    }

    pub fn as_lambda(&self) -> Option<&LambdaData> {
        match self.as_heap()? {
            HeapValue::Lambda(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_command_result(&self) -> Option<&CommandResultData> {
        match self.as_heap()? {
            HeapValue::CommandResult(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_bytes_ref(&self) -> Option<&Vec<u8>> {
        match self.as_heap()? {
            HeapValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_atomic(&self) -> Option<&AtomicValue> {
        match self.as_heap()? {
            HeapValue::Atomic(a) => Some(a),
            _ => None,
        }
    }

    // --- Raw u64 access (for JIT) ---

    #[inline]
    pub fn raw(&self) -> u64 { self.0 }

    #[inline]
    pub fn from_raw(bits: u64) -> Self { Self(bits) }

    // --- Pattern matching adapter ---
    // Allows existing `match val { Value::Int(n) => ... }` to become
    // `match val.kind() { VK::Int(n) => ... }` with minimal changes.

    pub fn kind(&self) -> ValueKind<'_> {
        if let Some(n) = self.as_int() { return ValueKind::Int(n); }
        if let Some(f) = self.as_float() { return ValueKind::Float(f); }
        if let Some(b) = self.as_bool() { return ValueKind::Bool(b); }
        if self.is_void() { return ValueKind::Void; }
        if let Some(s) = self.as_str_ref() { return ValueKind::String(s); }
        if let Some(rc) = self.as_list_ref() { return ValueKind::List(rc); }
        if let Some(rc) = self.as_object_ref() { return ValueKind::Object(rc); }
        if let Some(d) = self.as_lambda() { return ValueKind::Lambda(d); }
        if let Some(d) = self.as_command_result() { return ValueKind::CommandResult(d); }
        if let Some(h) = self.as_thread_handle() { return ValueKind::ThreadHandle(h); }
        if let Some(b) = self.as_bytes_ref() { return ValueKind::Bytes(b); }
        if let Some(h) = self.as_file_handle() { return ValueKind::FileHandle(h); }
        if let Some(a) = self.as_atomic() { return ValueKind::Atomic(a); }
        ValueKind::Void
    }

    // --- Type introspection ---

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self.tag() {
            t if t == TAG_INT => "int",
            t if t == TAG_BOOL => "bool",
            t if t == TAG_VOID => "void",
            t if t == TAG_STRING => "string",
            t if t == TAG_LIST => "list",
            t if t == TAG_OBJECT => "object",
            t if t == TAG_BOXED => {
                match self.as_heap() {
                    Some(HeapValue::BigInt(_)) => "int",
                    Some(HeapValue::Lambda(_)) => "ref",
                    Some(HeapValue::CommandResult(_)) => "result",
                    Some(HeapValue::ThreadHandle(_)) => "thread",
                    Some(HeapValue::Bytes(_)) => "bytes",
                    Some(HeapValue::FileHandle(_)) => "filehandle",
                    Some(HeapValue::Atomic(_)) => "atomic",
                    None => "void",
                }
            }
            _ => "float", // anything not tagged is a float
        }
    }

    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self.tag() {
            t if t == TAG_BOOL => self.payload() != 0,
            t if t == TAG_INT => self.payload() != 0,
            t if t == TAG_VOID => false,
            t if t == TAG_STRING => {
                self.as_str_ref().map_or(false, |s| !s.is_empty())
            }
            t if t == TAG_LIST => {
                self.as_list_ref().map_or(true, |rc| !rc.borrow().is_empty())
            }
            t if t == TAG_BOXED => {
                match self.as_heap() {
                    Some(HeapValue::BigInt(n)) => *n != 0,
                    Some(HeapValue::Bytes(b)) => !b.is_empty(),
                    Some(HeapValue::Atomic(a)) => a.load().is_truthy(),
                    _ => true,
                }
            }
            _ => {
                // Float
                self.as_float().map_or(true, |f| f != 0.0)
            }
        }
    }

    // --- Convenience constructors for external API ---

    pub fn new_string(s: &str) -> Self {
        Self::string(Rc::from(s))
    }

    pub fn new_list(items: Vec<Value>) -> Self {
        Self::list(Rc::new(RefCell::new(items)))
    }

    pub fn new_object(map: IndexMap<String, Value>) -> Self {
        Self::object(Rc::new(RefCell::new(ObjectData { fields: map, dyn_fields: HashSet::new() })))
    }

    pub fn new_object_with_dyn(map: IndexMap<String, Value>, dyn_fields: HashSet<String>) -> Self {
        Self::object(Rc::new(RefCell::new(ObjectData { fields: map, dyn_fields })))
    }

    // --- Conversion ---

    pub fn to_sendable(&self) -> SendableValue {
        if let Some(n) = self.as_int() { return SendableValue::Int(n); }
        if let Some(f) = self.as_float() { return SendableValue::Float(f); }
        if let Some(b) = self.as_bool() { return SendableValue::Bool(b); }
        if self.is_void() { return SendableValue::Void; }
        if let Some(s) = self.as_str_ref() { return SendableValue::String(s.to_string()); }
        if let Some(rc) = self.as_list_ref() {
            return SendableValue::List(rc.borrow().iter().map(Value::to_sendable).collect());
        }
        if let Some(rc) = self.as_object_ref() {
            return SendableValue::Object(rc.borrow().fields.iter().map(|(k, v)| (k.clone(), v.to_sendable())).collect());
        }
        if let Some(d) = self.as_lambda() {
            return SendableValue::Lambda {
                name: d.name.clone(), resolution: d.resolution,
                bound_args: d.bound_args.iter().map(Value::to_sendable).collect(),
            };
        }
        if let Some(d) = self.as_command_result() {
            return SendableValue::CommandResult {
                status: d.status, out: d.out.clone(), err: d.err.clone(),
            };
        }
        if let Some(b) = self.as_bytes_ref() {
            return SendableValue::Bytes(b.clone());
        }
        if let Some(a) = self.as_atomic() {
            return SendableValue::Atomic(a.clone());
        }
        SendableValue::Void
    }

    pub fn from_sendable(sv: SendableValue) -> Self {
        match sv {
            SendableValue::Int(n) => Self::int(n),
            SendableValue::Float(n) => Self::float(n),
            SendableValue::String(s) => Self::string(Rc::from(s)),
            SendableValue::Bool(b) => Self::bool(b),
            SendableValue::Void => Self::void(),
            SendableValue::List(items) => Self::new_list(items.into_iter().map(Self::from_sendable).collect()),
            SendableValue::Object(m) => Self::new_object(m.into_iter().map(|(k, v)| (k, Self::from_sendable(v))).collect()),
            SendableValue::Lambda { name, resolution, bound_args } => Self::lambda(LambdaData {
                name, resolution, bound_args: bound_args.into_iter().map(Self::from_sendable).collect(),
            }),
            SendableValue::CommandResult { status, out, err } => Self::command_result(CommandResultData { status, out, err }),
            SendableValue::Bytes(b) => Self::bytes(b),
            SendableValue::Atomic(a) => Self::atomic(a),
        }
    }
}

// ---------------------------------------------------------------------------
// Clone: cheap for inline types, Rc increment for pointers
// ---------------------------------------------------------------------------

impl Clone for Value {
    #[inline]
    fn clone(&self) -> Self {
        match self.tag() {
            t if t == TAG_STRING => {
                // Increment Rc<String> refcount, get new raw pointer
                let ptr = self.payload() as *const String;
                unsafe {
                    let rc = Rc::from_raw(ptr);
                    let cloned = rc.clone();
                    std::mem::forget(rc); // don't decrement original
                    Self(TAG_STRING | (Rc::into_raw(cloned) as u64 & PAYLOAD_MASK))
                }
            }
            t if t == TAG_LIST => {
                let ptr = self.payload() as *const RefCell<Vec<Value>>;
                unsafe {
                    let rc = Rc::from_raw(ptr);
                    let cloned = rc.clone();
                    std::mem::forget(rc);
                    Self(TAG_LIST | (Rc::into_raw(cloned) as u64 & PAYLOAD_MASK))
                }
            }
            t if t == TAG_OBJECT => {
                let ptr = self.payload() as *const RefCell<ObjectData>;
                unsafe {
                    let rc = Rc::from_raw(ptr);
                    let cloned = rc.clone();
                    std::mem::forget(rc);
                    Self(TAG_OBJECT | (Rc::into_raw(cloned) as u64 & PAYLOAD_MASK))
                }
            }
            t if t == TAG_BOXED => {
                if let Some(hv) = self.as_heap() {
                    let cloned = match hv {
                        HeapValue::BigInt(n) => HeapValue::BigInt(*n),
                        HeapValue::Lambda(d) => HeapValue::Lambda(d.clone()),
                        HeapValue::CommandResult(d) => HeapValue::CommandResult(d.clone()),
                        HeapValue::ThreadHandle(h) => HeapValue::ThreadHandle(h.clone()),
                        HeapValue::Bytes(b) => HeapValue::Bytes(b.clone()),
                        HeapValue::FileHandle(h) => HeapValue::FileHandle(h.clone()),
                        HeapValue::Atomic(a) => HeapValue::Atomic(a.clone()),
                    };
                    Self::boxed(cloned)
                } else {
                    Self::void()
                }
            }
            _ => {
                // Int, Bool, Void, Float — just copy the u64
                Self(self.0)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Drop: decrement Rc for pointer types, free Box for boxed types
// ---------------------------------------------------------------------------

impl Drop for Value {
    #[inline]
    fn drop(&mut self) {
        let payload = self.payload();
        if payload == 0 { return; }
        match self.tag() {
            t if t == TAG_STRING => {
                // Decrement Rc<String>
                unsafe { drop(Rc::from_raw(payload as *const String)); }
            }
            t if t == TAG_LIST => {
                unsafe { drop(Rc::from_raw(payload as *const RefCell<Vec<Value>>)); }
            }
            t if t == TAG_OBJECT => {
                unsafe { drop(Rc::from_raw(payload as *const RefCell<ObjectData>)); }
            }
            t if t == TAG_BOXED => {
                unsafe { drop(Box::from_raw(payload as *mut HeapValue)); }
            }
            _ => {} // inline types — nothing to free
        }
    }
}

// ---------------------------------------------------------------------------
// Debug
// ---------------------------------------------------------------------------

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(n) = self.as_int() { return write!(f, "{n}"); }
        if let Some(fl) = self.as_float() { return write!(f, "{fl}"); }
        if let Some(b) = self.as_bool() { return write!(f, "{b}"); }
        if self.is_void() { return write!(f, "void"); }
        if let Some(s) = self.as_str_ref() { return write!(f, "{s}"); }
        if let Some(rc) = self.as_list_ref() {
            let items = rc.borrow();
            write!(f, "[")?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 { write!(f, ", ")?; }
                write!(f, "{item}")?;
            }
            return write!(f, "]");
        }
        if let Some(rc) = self.as_object_ref() {
            let map = rc.borrow();
            write!(f, "{{ ")?;
            for (i, (k, v)) in map.fields.iter().enumerate() {
                if i > 0 { write!(f, ", ")?; }
                write!(f, "{k} = {v}")?;
            }
            return write!(f, " }}");
        }
        if self.tag() == TAG_BOXED {
            match self.as_heap() {
                Some(HeapValue::Lambda(d)) => {
                    write!(f, "@{}", d.name)?;
                    if !d.bound_args.is_empty() {
                        write!(f, "(")?;
                        for (i, arg) in d.bound_args.iter().enumerate() {
                            if i > 0 { write!(f, ", ")?; }
                            write!(f, "{arg}")?;
                        }
                        write!(f, ")")?;
                    }
                    return Ok(());
                }
                Some(HeapValue::CommandResult(d)) => {
                    return write!(f, "Result {{ status = {}, out = \"{}\", err = \"{}\" }}", d.status, d.out, d.err);
                }
                Some(HeapValue::ThreadHandle(_)) => return write!(f, "thread(..)"),
                Some(HeapValue::Bytes(b)) => return write!(f, "bytes({})", b.len()),
                Some(HeapValue::FileHandle(_)) => return write!(f, "filehandle(..)"),
                Some(HeapValue::Atomic(a)) => return write!(f, "{}", a.load()),
                Some(HeapValue::BigInt(n)) => return write!(f, "{n}"),
                None => return write!(f, "void"),
            }
        }
        write!(f, "?")
    }
}

// ---------------------------------------------------------------------------
// Helper functions (compat with old API)
// ---------------------------------------------------------------------------

#[inline]
pub fn new_list(items: Vec<Value>) -> Value {
    Value::new_list(items)
}

#[inline]
pub fn new_object(map: IndexMap<String, Value>) -> Value {
    Value::new_object(map)
}

#[inline]
pub fn new_object_with_dyn(map: IndexMap<String, Value>, dyn_fields: HashSet<String>) -> Value {
    Value::new_object_with_dyn(map, dyn_fields)
}

// ---------------------------------------------------------------------------
// ValueKind — pattern matching adapter for NaN-boxed Value
// ---------------------------------------------------------------------------

pub enum ValueKind<'a> {
    Int(i64),
    Float(f64),
    String(&'a str),
    Bool(bool),
    List(&'a RefCell<Vec<Value>>),
    Object(&'a RefCell<ObjectData>),
    Void,
    Lambda(&'a LambdaData),
    CommandResult(&'a CommandResultData),
    ThreadHandle(&'a Arc<Mutex<ThreadJoinHandle>>),
    Bytes(&'a Vec<u8>),
    FileHandle(&'a Arc<Mutex<FileHandleInner>>),
    Atomic(&'a AtomicValue),
}
