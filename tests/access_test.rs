use delegate_shell::{Interpreter, BuiltinAccess};

#[test]
fn all_mode_has_everything() {
    let mut engine = Interpreter::new().unwrap();
    assert!(engine.run_source("println(\"hello\")").is_ok());
    assert!(engine.run_source("a1 = len(\"test\")").is_ok());
    assert!(engine.run_source("a2 = str(42)").is_ok());
}

#[test]
fn core_mode_blocks_io() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    // println is IO, should fail
    let result = engine.run_source("println(\"should fail\")");
    assert!(result.is_err(), "println should be blocked in Core mode");
}

#[test]
fn core_mode_blocks_filesystem() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    let result = engine.run_source("read(\"/tmp/test\")");
    assert!(result.is_err(), "read should be blocked in Core mode");
}

#[test]
fn core_mode_blocks_network() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    let result = engine.run_source("http_get(\"http://example.com\")");
    assert!(result.is_err(), "http_get should be blocked in Core mode");
}

#[test]
fn core_mode_blocks_hashing() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    let result = engine.run_source("sha256(\"test\")");
    assert!(result.is_err(), "sha256 should be blocked in Core mode");
}

#[test]
fn core_mode_allows_types() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    assert!(engine.run_source("t1 = str(42)").is_ok());
    assert!(engine.run_source("t2 = int(\"123\")").is_ok());
    assert!(engine.run_source("t3 = float(\"3.14\")").is_ok());
    assert!(engine.run_source("t4 = bool(1)").is_ok());
    assert!(engine.run_source("t5 = type(42)").is_ok());
}

#[test]
fn core_mode_allows_strings() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    assert!(engine.run_source("s1 = len(\"hello\")").is_ok());
    assert!(engine.run_source("s2 = trim(\"  hi  \")").is_ok());
    assert!(engine.run_source("s3 = upper(\"hello\")").is_ok());
    assert!(engine.run_source("s4 = split(\"a,b\", \",\")").is_ok());
}

#[test]
fn core_mode_allows_collections() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    assert!(engine.run_source("c1 = [1,2,3]\npush(c1, 4)").is_ok());
    assert!(engine.run_source("c2 = [3,1,2]\nc2s = sort(c2)").is_ok());
    assert!(engine.run_source("c3 = [1,2,3]\nc3l = len(c3)").is_ok());
}

#[test]
fn core_mode_allows_math() {
    let mut engine = Interpreter::with_access(BuiltinAccess::Core).unwrap();
    assert!(engine.run_source("m1 = abs_num(-5.0)").is_ok());
    assert!(engine.run_source("m2 = round(3.7)").is_ok());
    assert!(engine.run_source("m3 = sqrt(16.0)").is_ok());
}

#[test]
fn none_mode_blocks_everything() {
    let mut engine = Interpreter::with_access(BuiltinAccess::None).unwrap();
    assert!(engine.run_source("println(\"fail\")").is_err());
    assert!(engine.run_source("x = len(\"fail\")").is_err());
    assert!(engine.run_source("x = str(42)").is_err());
}

#[test]
fn none_mode_allows_language_features() {
    let mut engine = Interpreter::with_access(BuiltinAccess::None).unwrap();
    // Pure language: variables, functions, control flow
    assert!(engine.run_source("x = 42").is_ok());
    assert!(engine.run_source("if true\n    x = 1").is_ok());
    assert!(engine.run_source("add(a, b)\n    return a + b\nx = add!(1, 2)").is_ok());
}

#[test]
fn none_mode_allows_registered_functions() {
    let mut engine = Interpreter::with_access(BuiltinAccess::None).unwrap();
    engine.register(
        "my_func",
        &[],
        delegate_shell::Type::Int,
        |_, _| Ok(delegate_shell::Value::int(42)),
    ).unwrap();
    assert!(engine.run_source("x = my_func!!()").is_ok());
}

#[test]
fn sandboxed_blocks_exec() {
    let mut engine = Interpreter::sandboxed().unwrap();
    assert!(!engine.allow_exec());
}

#[test]
fn register_override_replaces_builtin() {
    let mut engine = Interpreter::new().unwrap();
    engine.register_override(
        "len",
        &[delegate_shell::Param::Required(delegate_shell::Type::Dyn)],
        delegate_shell::Type::Int,
        |_, _| Ok(delegate_shell::Value::int(999)),
    ).unwrap();
    // len should now return 999 for anything
    assert!(engine.run_source("x = len(\"hello\")").is_ok());
}
