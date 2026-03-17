use std::collections::HashMap;
use crate::interpreter::value::MaybeError;
use crate::parser::ast::Stmt;

/// A user-defined function
#[derive(Debug, Clone)]
pub struct UserFn {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
}

pub struct Environment {
    /// Variable scopes — innermost last
    scopes: Vec<HashMap<String, MaybeError>>,
    /// User-defined functions
    pub functions: HashMap<String, UserFn>,
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
    ///
    /// # Panics
    ///
    /// Panics if there are no scopes (should never happen in normal usage).
    pub fn set(&mut self, name: &str, value: MaybeError) {
        let key = name.to_ascii_lowercase();
        // Check if variable exists in any scope — update it there
        for scope in self.scopes.iter_mut().rev() {
            if let std::collections::hash_map::Entry::Occupied(mut entry) = scope.entry(key.clone()) {
                entry.insert(value);
                return;
            }
        }
        // Otherwise create in current scope
        self.scopes.last_mut().expect("scopes should never be empty").insert(key, value);
    }

    /// Set a variable strictly in the current (innermost) scope — used for function parameters.
    ///
    /// # Panics
    ///
    /// Panics if there are no scopes (should never happen in normal usage).
    pub fn set_local(&mut self, name: &str, value: MaybeError) {
        let key = name.to_ascii_lowercase();
        self.scopes.last_mut().expect("scopes should never be empty").insert(key, value);
    }

    /// Get a variable — searches from innermost to outermost scope
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&MaybeError> {
        let key = name.to_ascii_lowercase();
        for scope in self.scopes.iter().rev() {
            if let Some(val) = scope.get(&key) {
                return Some(val);
            }
        }
        None
    }

    /// Define a function
    pub fn define_fn(&mut self, func: UserFn) {
        self.functions.insert(func.name.to_ascii_lowercase(), func);
    }

    /// Look up a user-defined function
    #[must_use]
    pub fn get_fn(&self, name: &str) -> Option<&UserFn> {
        self.functions.get(&name.to_ascii_lowercase())
    }
}
