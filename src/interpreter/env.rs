use std::collections::{HashMap, HashSet};
use std::borrow::Cow;
use crate::interpreter::value::{MaybeError, ObjectData, Value};
use crate::parser::ast::{Stmt, TypeAnnotation};

/// Lowercase a string, avoiding allocation if it's already lowercase.
#[inline]
#[must_use]
pub fn to_lower_pub(s: &str) -> Cow<'_, str> {
    to_lower(s)
}

#[inline]
fn to_lower(s: &str) -> Cow<'_, str> {
    if s.bytes().all(|b| !b.is_ascii_uppercase()) {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(s.to_ascii_lowercase())
    }
}

/// A user-defined function
#[derive(Debug, Clone)]
pub struct UserFn {
    pub name: String,
    /// Required parameters: (name, type_annotation, is_dyn)
    pub params: Vec<(String, Option<TypeAnnotation>, bool)>,
    /// Optional parameters: (name, type_annotation, is_dyn)
    pub optional_params: Vec<(String, Option<TypeAnnotation>, bool)>,
    pub body: Vec<Stmt>,
    /// Declared return type annotation (from the function definition)
    pub declared_return_type: Option<TypeAnnotation>,
    /// Whether the function uses `dyn return` — allows different return types per call
    pub has_dyn_return: bool,
    /// Inferred parameter types: param_name → TypeAnnotation.
    /// Populated by body analysis at definition time, refined by first call.
    pub inferred_types: HashMap<String, TypeAnnotation>,
    /// Params whose type was inferred from body (vs call-site) — for error messages
    pub body_inferred_params: HashSet<String>,
    /// Inferred return type name, set on first call based on outermost return (legacy)
    pub return_type: Option<String>,
}

pub struct Environment {
    /// Variable scopes — innermost last
    scopes: Vec<HashMap<String, MaybeError>>,
    /// User-defined functions
    pub functions: HashMap<String, UserFn>,
    /// Paths registered via `use` — maps alias/name to full path
    pub use_paths: HashMap<String, String>,
    /// Name aliases — maps a call name to its target executable string
    pub aliases: HashMap<String, String>,
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    #[must_use]
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            use_paths: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Set a variable — updates existing in any scope, or creates in current scope.
    /// Enforces type consistency: once a variable has a type, it cannot change.
    /// Use `free` to release a variable before reassigning with a different type.
    ///
    /// # Errors
    ///
    /// Returns an error if the new value's type doesn't match the existing variable's type.
    ///
    /// # Panics
    ///
    /// Panics if the scopes stack is empty (should never happen).
    pub fn set(&mut self, name: &str, value: MaybeError) -> Result<(), String> {
        let key = to_lower(name);
        // Check if variable exists in any scope — update it there
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(key.as_ref()) {
                // If existing is Atomic, store into it instead of replacing
                if let (MaybeError::Ok(Value::Atomic(a)), MaybeError::Ok(new_val)) = (&slot, &value) {
                    a.store(new_val);
                    return Ok(());
                }
                // Type check: existing Ok value must match new Ok value's type
                if let (MaybeError::Ok(existing), MaybeError::Ok(new_val)) = (&slot, &value) {
                    let old_type = existing.type_name();
                    let new_type = new_val.type_name();
                    if old_type != new_type && new_type != "atomic" {
                        return Err(format!(
                            "Type mismatch: variable '{name}' is {old_type}, cannot assign {new_type} (use 'free {name}' first)"
                        ));
                    }
                    // Structural check for objects: same fields with same types
                    if let (Value::Object(old_rc), Value::Object(new_rc)) = (existing, new_val) {
                        check_object_structure(name, &*old_rc.borrow(), &*new_rc.borrow())?;
                    }
                }
                *slot = value;
                return Ok(());
            }
        }
        // Otherwise create in current scope
        let scope = self.scopes.last_mut()
            .ok_or_else(|| "Internal error: no scope available".to_string())?;
        scope.insert(key.into_owned(), value);
        Ok(())
    }

    /// Set a variable — `dyn` flag is a marker for future static analysis, no runtime effect.
    pub fn set_dyn(&mut self, name: &str, value: MaybeError, _is_dyn: bool) -> Result<(), String> {
        self.set(name, value)
    }

    /// Set a variable strictly in the current (innermost) scope — used for function parameters.
    pub fn set_local(&mut self, name: &str, value: MaybeError) {
        let key = to_lower(name);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(key.into_owned(), value);
        }
    }

    /// Get a variable — searches from innermost to outermost scope
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&MaybeError> {
        let key = to_lower(name);
        for scope in self.scopes.iter().rev() {
            if let Some(val) = scope.get(key.as_ref()) {
                return Some(val);
            }
        }
        None
    }

    /// Remove a variable from all scopes. Returns true if found and removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let key = to_lower(name);
        for scope in self.scopes.iter_mut().rev() {
            if scope.remove(key.as_ref()).is_some() {
                // Shrink only when capacity is more than 4x the length
                // Avoids realloc on every free
                if scope.capacity() > scope.len() * 4 + 16 {
                    scope.shrink_to_fit();
                }
                return true;
            }
        }
        false
    }

    /// Define a function
    pub fn define_fn(&mut self, func: UserFn) {
        self.functions.insert(func.name.to_ascii_lowercase(), func);
    }

    /// Look up a user-defined function
    #[must_use]
    pub fn get_fn(&self, name: &str) -> Option<&UserFn> {
        let key = to_lower(name);
        self.functions.get(key.as_ref())
    }

    /// Clone all user functions (for thread spawning)
    #[must_use]
    pub fn clone_fns(&self) -> HashMap<String, UserFn> {
        self.functions.clone()
    }

    /// Restore user functions (in a new thread interpreter)
    pub fn restore_fns(&mut self, fns: HashMap<String, UserFn>) {
        self.functions = fns;
    }
}

/// Check that two objects have the same structure: same field names with same value types.
fn check_object_structure(
    var_name: &str,
    old: &ObjectData,
    new: &ObjectData,
) -> Result<(), String> {
    // Check for missing fields
    for key in old.fields.keys() {
        if !new.fields.contains_key(key) {
            return Err(format!(
                "Type mismatch: variable '{var_name}' object is missing field '{key}'"
            ));
        }
    }
    // Check for extra fields
    for key in new.fields.keys() {
        if !old.fields.contains_key(key) {
            return Err(format!(
                "Type mismatch: variable '{var_name}' object has unexpected field '{key}'"
            ));
        }
    }
    // Check field types match
    for (key, old_val) in &old.fields {
        let new_val = &new.fields[key];
        // Skip type check for dyn fields when the new value is Void
        if new.dyn_fields.contains(key) && matches!(new_val, Value::Void) {
            continue;
        }
        let old_type = old_val.type_name();
        let new_type = new_val.type_name();
        if old_type != new_type {
            return Err(format!(
                "Type mismatch: field '{key}' of '{var_name}' is {old_type}, cannot assign {new_type}"
            ));
        }
        // Recursive check for nested objects
        if let (Value::Object(old_rc), Value::Object(new_rc)) = (old_val, new_val) {
            check_object_structure(&format!("{var_name}.{key}"), &*old_rc.borrow(), &*new_rc.borrow())?;
        }
    }
    Ok(())
}
