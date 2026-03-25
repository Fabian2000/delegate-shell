//! AOT code generation: compiles bytecode chunks to a native object file using Cranelift ObjectModule.

use cranelift_codegen::ir::{types, AbiParam, Function, InstBuilder, UserFuncName, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_object::{ObjectBuilder, ObjectModule};
use cranelift_module::{DataDescription, Linkage, Module};

use std::collections::{HashMap, HashSet};
use crate::vm::bytecode::{self, Chunk, Op};
use crate::interpreter::value::Value;

// ---------------------------------------------------------------------------
// Helper function name constants (same as jit.rs — these become external symbols)
// ---------------------------------------------------------------------------

const H_BINARY_OP: &str = "jit_binary_op";
const H_NEGATE: &str = "jit_negate";
const H_BITNOT: &str = "jit_bitnot";
const H_NOT: &str = "jit_not";
const H_POW: &str = "jit_pow";
const H_IS_TRUTHY: &str = "jit_is_truthy";
const H_CLONE_VALUE: &str = "jit_clone_value";
const H_DROP_VALUE: &str = "jit_drop_value";
const H_GET_GLOBAL: &str = "jit_get_global";
const H_SET_GLOBAL: &str = "jit_set_global";
const H_FIELD_GET: &str = "jit_field_get";
const H_FIELD_SET: &str = "jit_field_set";
const H_INDEX_GET: &str = "jit_index_get";
const H_INDEX_SET: &str = "jit_index_set";
const H_MAKE_LIST: &str = "jit_make_list";
const H_MAKE_OBJECT: &str = "jit_make_object";
const H_MAKE_STRING: &str = "jit_make_string";
const H_MAKE_RANGE: &str = "jit_make_range";
const H_GENERIC_CALL: &str = "jit_generic_call";
const H_CALL_BUILTIN: &str = "jit_call_builtin_v2";
const H_MAKE_LAMBDA: &str = "jit_make_lambda";
const H_INC_DEC: &str = "jit_inc_dec";
const H_COMPOUND_OP: &str = "jit_compound_op";
const H_STRING_APPEND_LOCAL: &str = "jit_string_append_local";
const H_VM_CALL: &str = "jit_vm_call";
const H_DEFINE_FUNCTION: &str = "jit_define_function";
const H_FREE_GLOBAL: &str = "jit_free_global";
const H_THROW: &str = "jit_throw";
const H_RECURSION_OVERFLOW: &str = "jit_recursion_overflow";
const H_ERROR_CHECK: &str = "jit_error_check";
const H_ERROR_FIELD: &str = "jit_error_field";
const H_SET_ERROR_TOLERANT: &str = "jit_set_error_tolerant";
const H_RECORD_ERROR: &str = "jit_record_error";
const H_OPTIONAL_CHECK: &str = "jit_optional_check";
const H_GET_DOLLAR_INDEX: &str = "jit_get_dollar_index";
const H_GET_DOLLAR_FIELD: &str = "jit_get_dollar_field";
const H_GET_DOLLAR: &str = "jit_get_dollar";
const H_PUSH_SEND_CTX: &str = "jit_push_send_ctx";
const H_POP_SEND_CTX: &str = "jit_pop_send_ctx";
const H_ALIAS: &str = "jit_alias";
const H_USE: &str = "jit_use";
const H_ATOMIC: &str = "jit_atomic";

// AOT-specific helpers
const H_AOT_INIT: &str = "dgsh_aot_init";
const H_AOT_CLEANUP: &str = "dgsh_aot_cleanup";

// ---------------------------------------------------------------------------
// NaN-boxing constants for inline fast paths
// ---------------------------------------------------------------------------
const NB_TAG_INT: u64      = 0x7FF8_0000_0000_0000;
const NB_TAG_BOOL: u64     = 0x7FF9_0000_0000_0000;
const NB_TAG_MASK: u64     = 0xFFFF_0000_0000_0000;
const NB_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

// BOP codes
const BOP_ADD: u64 = 0;
const BOP_SUB: u64 = 1;
const BOP_MUL: u64 = 2;
const BOP_DIV: u64 = 3;
const BOP_MOD: u64 = 4;
const BOP_EQ: u64 = 5;
const BOP_NEQ: u64 = 6;
const BOP_LT: u64 = 7;
const BOP_GT: u64 = 8;
const BOP_LTE: u64 = 9;
const BOP_GTE: u64 = 10;
const BOP_BITAND: u64 = 11;
const BOP_BITOR: u64 = 12;
const BOP_BITXOR: u64 = 13;
const BOP_SHL: u64 = 14;
const BOP_SHR: u64 = 15;

/// What the inline int fast-path should compute.
#[derive(Clone, Copy)]
enum IntFastOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Neq, Lt, Gt, Lte, Gte,
}

// ---------------------------------------------------------------------------
// Helper function references in Cranelift IR (for ObjectModule)
// ---------------------------------------------------------------------------

struct HelperRefs {
    binary_op: cranelift_codegen::ir::FuncRef,
    negate: cranelift_codegen::ir::FuncRef,
    bitnot: cranelift_codegen::ir::FuncRef,
    not: cranelift_codegen::ir::FuncRef,
    pow: cranelift_codegen::ir::FuncRef,
    is_truthy: cranelift_codegen::ir::FuncRef,
    clone_value: cranelift_codegen::ir::FuncRef,
    get_global: cranelift_codegen::ir::FuncRef,
    set_global: cranelift_codegen::ir::FuncRef,
    field_get: cranelift_codegen::ir::FuncRef,
    field_set: cranelift_codegen::ir::FuncRef,
    index_get: cranelift_codegen::ir::FuncRef,
    index_set: cranelift_codegen::ir::FuncRef,
    make_list: cranelift_codegen::ir::FuncRef,
    make_object: cranelift_codegen::ir::FuncRef,
    make_string: cranelift_codegen::ir::FuncRef,
    make_range: cranelift_codegen::ir::FuncRef,
    generic_call: cranelift_codegen::ir::FuncRef,
    call_builtin: cranelift_codegen::ir::FuncRef,
    make_lambda: cranelift_codegen::ir::FuncRef,
    inc_dec: cranelift_codegen::ir::FuncRef,
    compound_op: cranelift_codegen::ir::FuncRef,
    string_append_local: cranelift_codegen::ir::FuncRef,
    vm_call: cranelift_codegen::ir::FuncRef,
    define_function: cranelift_codegen::ir::FuncRef,
    free_global: cranelift_codegen::ir::FuncRef,
    throw: cranelift_codegen::ir::FuncRef,
    recursion_overflow: cranelift_codegen::ir::FuncRef,
    error_check: cranelift_codegen::ir::FuncRef,
    error_field: cranelift_codegen::ir::FuncRef,
    set_error_tolerant: cranelift_codegen::ir::FuncRef,
    record_error: cranelift_codegen::ir::FuncRef,
    optional_check: cranelift_codegen::ir::FuncRef,
    get_dollar_index: cranelift_codegen::ir::FuncRef,
    get_dollar_field: cranelift_codegen::ir::FuncRef,
    get_dollar: cranelift_codegen::ir::FuncRef,
    push_send_ctx: cranelift_codegen::ir::FuncRef,
    pop_send_ctx: cranelift_codegen::ir::FuncRef,
    alias: cranelift_codegen::ir::FuncRef,
    use_fn: cranelift_codegen::ir::FuncRef,
    atomic: cranelift_codegen::ir::FuncRef,
    aot_load_const: cranelift_codegen::ir::FuncRef,
    aot_set_chunk: cranelift_codegen::ir::FuncRef,
}

impl HelperRefs {
    fn declare(module: &mut ObjectModule, func: &mut Function) -> Option<Self> {
        let i64t = types::I64;

        macro_rules! decl {
            ($name:expr, [$($p:expr),*], [$($r:expr),*]) => {{
                let mut sig = module.make_signature();
                $(sig.params.push(AbiParam::new($p));)*
                $(sig.returns.push(AbiParam::new($r));)*
                let id = module.declare_function($name, Linkage::Import, &sig).ok()?;
                module.declare_func_in_func(id, func)
            }};
        }

        Some(Self {
            binary_op: decl!(H_BINARY_OP, [i64t, i64t, i64t], [i64t]),
            negate: decl!(H_NEGATE, [i64t], [i64t]),
            bitnot: decl!(H_BITNOT, [i64t], [i64t]),
            not: decl!(H_NOT, [i64t], [i64t]),
            pow: decl!(H_POW, [i64t, i64t], [i64t]),
            is_truthy: decl!(H_IS_TRUTHY, [i64t], [i64t]),
            clone_value: decl!(H_CLONE_VALUE, [i64t], [i64t]),
            get_global: decl!(H_GET_GLOBAL, [i64t], [i64t]),
            set_global: decl!(H_SET_GLOBAL, [i64t, i64t], []),
            field_get: decl!(H_FIELD_GET, [i64t, i64t], [i64t]),
            field_set: decl!(H_FIELD_SET, [i64t, i64t, i64t], []),
            index_get: decl!(H_INDEX_GET, [i64t, i64t], [i64t]),
            index_set: decl!(H_INDEX_SET, [i64t, i64t, i64t], []),
            make_list: decl!(H_MAKE_LIST, [i64t, i64t], [i64t]),
            make_object: decl!(H_MAKE_OBJECT, [i64t, i64t], [i64t]),
            make_string: decl!(H_MAKE_STRING, [i64t, i64t], [i64t]),
            make_range: decl!(H_MAKE_RANGE, [i64t, i64t], [i64t]),
            generic_call: decl!(H_GENERIC_CALL, [i64t, i64t, i64t, i64t], [i64t]),
            call_builtin: decl!(H_CALL_BUILTIN, [i64t, i64t, i64t], [i64t]),
            make_lambda: decl!(H_MAKE_LAMBDA, [i64t, i64t, i64t, i64t], [i64t]),
            inc_dec: decl!(H_INC_DEC, [i64t, i64t], [i64t]),
            compound_op: decl!(H_COMPOUND_OP, [i64t, i64t, i64t], [i64t]),
            string_append_local: decl!(H_STRING_APPEND_LOCAL, [i64t, i64t], [i64t]),
            vm_call: decl!(H_VM_CALL, [i64t, i64t, i64t], [i64t]),
            define_function: decl!(H_DEFINE_FUNCTION, [i64t, i64t], []),
            free_global: decl!(H_FREE_GLOBAL, [i64t], []),
            throw: decl!(H_THROW, [i64t], [i64t]),
            recursion_overflow: decl!(H_RECURSION_OVERFLOW, [], []),
            error_check: decl!(H_ERROR_CHECK, [i64t], [i64t]),
            error_field: decl!(H_ERROR_FIELD, [i64t], [i64t]),
            set_error_tolerant: decl!(H_SET_ERROR_TOLERANT, [i64t], []),
            record_error: decl!(H_RECORD_ERROR, [i64t], []),
            optional_check: decl!(H_OPTIONAL_CHECK, [i64t], [i64t]),
            get_dollar_index: decl!(H_GET_DOLLAR_INDEX, [i64t], [i64t]),
            get_dollar_field: decl!(H_GET_DOLLAR_FIELD, [i64t], [i64t]),
            get_dollar: decl!(H_GET_DOLLAR, [], [i64t]),
            push_send_ctx: decl!(H_PUSH_SEND_CTX, [i64t], []),
            pop_send_ctx: decl!(H_POP_SEND_CTX, [], []),
            alias: decl!(H_ALIAS, [i64t, i64t], []),
            use_fn: decl!(H_USE, [i64t, i64t], []),
            atomic: decl!(H_ATOMIC, [i64t], [i64t]),
            aot_load_const: decl!("dgsh_aot_load_const", [i64t, i64t], [i64t]),
            aot_set_chunk: decl!("dgsh_aot_set_chunk", [i64t], []),
        })
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Compile all bytecode chunks to a native object file (ELF/Mach-O/COFF).
///
/// Returns the raw bytes of the object file.
pub fn compile_chunks_to_object(chunks: &[Chunk], teach_source: &str) -> Result<Vec<u8>, String> {
    // Check if the current architecture supports AOT compilation
    let arch = std::env::consts::ARCH;
    match arch {
        "x86_64" | "aarch64" => {}
        _ => return Err(format!(
            "AOT compilation is not supported on '{}'. Supported architectures: x86_64, aarch64. Use --vm instead.",
            arch
        )),
    }

    let mut flag_builder = settings::builder();
    let _ = flag_builder.set("use_colocated_libcalls", "false");
    let _ = flag_builder.set("is_pic", "false");
    let flags = settings::Flags::new(flag_builder);
    let isa = cranelift_native::builder()
        .map_err(|e| format!("Failed to create native ISA builder: {e}"))?
        .finish(flags)
        .map_err(|e| format!("Failed to finish ISA: {e}"))?;

    let builder = ObjectBuilder::new(
        isa,
        "dgsh_aot",
        cranelift_module::default_libcall_names(),
    ).map_err(|e| format!("Failed to create ObjectBuilder: {e}"))?;
    let mut module = ObjectModule::new(builder);
    let mut builder_ctx = FunctionBuilderContext::new();

    // Compile each chunk as a named function
    let mut chunk_func_ids = Vec::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        let func_name = format!("dgsh_chunk_{}", idx);

        // Build signature: for chunk 0 (top-level), no params; for others, params + depth
        let mut sig = module.make_signature();
        if chunk.param_count > 0 {
            for _ in 0..chunk.param_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.params.push(AbiParam::new(types::I64)); // depth counter
        }
        sig.returns.push(AbiParam::new(types::I64));

        let func_id = module.declare_function(&func_name, Linkage::Export, &sig)
            .map_err(|e| format!("Failed to declare function {func_name}: {e}"))?;
        chunk_func_ids.push(func_id);
    }

    // Define each chunk function
    for (idx, chunk) in chunks.iter().enumerate() {
        let func_id = chunk_func_ids[idx];
        // Rebuild signature (same as when we declared it)
        let mut sig = module.make_signature();
        if chunk.param_count > 0 {
            for _ in 0..chunk.param_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.params.push(AbiParam::new(types::I64)); // depth
        }
        sig.returns.push(AbiParam::new(types::I64));

        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let self_ref = module.declare_func_in_func(func_id, &mut func);
        let helpers = HelperRefs::declare(&mut module, &mut func)
            .ok_or_else(|| format!("Failed to declare helper refs for chunk {idx}"))?;

        // Build chunk_refs map: other chunk indices → FuncRef in this function
        let mut chunk_refs: HashMap<usize, cranelift_codegen::ir::FuncRef> = HashMap::new();
        for (other_idx, &other_func_id) in chunk_func_ids.iter().enumerate() {
            if other_idx != idx {
                let fref = module.declare_func_in_func(other_func_id, &mut func);
                chunk_refs.insert(other_idx, fref);
            }
        }

        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);
        let ok = compile_chunk(&mut builder, chunk, self_ref, &helpers, idx, &chunk_refs);
        builder.finalize();

        if !ok {
            return Err(format!("Failed to compile chunk {idx} ({})", chunk.name));
        }

        let mut ctx = Context::for_function(func);
        module.define_function(func_id, &mut ctx)
            .map_err(|e| format!("Failed to define function for chunk {idx}: {e}"))?;
        module.clear_context(&mut ctx);
    }

    // Embed serialized chunks as data section
    let chunks_data = bytecode::serialize_chunks(chunks);
    let chunks_data_id = module.declare_data("dgsh_chunks_data", Linkage::Export, false, false)
        .map_err(|e| format!("Failed to declare chunks data: {e}"))?;
    let mut data_desc = DataDescription::new();
    data_desc.define(chunks_data.into_boxed_slice());
    module.define_data(chunks_data_id, &data_desc)
        .map_err(|e| format!("Failed to define chunks data: {e}"))?;

    // Embed chunks data length
    let chunks_len_id = module.declare_data("dgsh_chunks_len", Linkage::Export, false, false)
        .map_err(|e| format!("Failed to declare chunks length: {e}"))?;
    let len_bytes = (bytecode::serialize_chunks(chunks).len() as u64).to_le_bytes();
    let mut len_desc = DataDescription::new();
    len_desc.define(len_bytes.to_vec().into_boxed_slice());
    module.define_data(chunks_len_id, &len_desc)
        .map_err(|e| format!("Failed to define chunks length: {e}"))?;

    // Embed teach source for runtime FFI initialization
    let teach_data_id = module.declare_data("dgsh_teach_data", Linkage::Export, false, false)
        .map_err(|e| format!("Failed to declare teach data: {e}"))?;
    let mut teach_desc = DataDescription::new();
    teach_desc.define(teach_source.as_bytes().to_vec().into_boxed_slice());
    module.define_data(teach_data_id, &teach_desc)
        .map_err(|e| format!("Failed to define teach data: {e}"))?;

    let teach_len_id = module.declare_data("dgsh_teach_len", Linkage::Export, false, false)
        .map_err(|e| format!("Failed to declare teach length: {e}"))?;
    let teach_len_bytes = (teach_source.len() as u64).to_le_bytes();
    let mut teach_len_desc = DataDescription::new();
    teach_len_desc.define(teach_len_bytes.to_vec().into_boxed_slice());
    module.define_data(teach_len_id, &teach_len_desc)
        .map_err(|e| format!("Failed to define teach length: {e}"))?;

    // Generate `main` entry point
    generate_main(&mut module, &mut builder_ctx, chunk_func_ids[0], chunks_data_id, chunks_len_id)?;

    let product = module.finish();
    let bytes = product.emit()
        .map_err(|e| format!("Failed to emit object file: {e}"))?;
    Ok(bytes)
}

/// Generate the `main` entry point function.
///
/// main():
///   1. load address of dgsh_chunks_data and dgsh_chunks_len
///   2. call dgsh_aot_init(data_ptr, data_len) -> returns context pointer
///   3. call dgsh_chunk_0() -> top-level code
///   4. call dgsh_aot_cleanup(ctx)
///   5. return 0
fn generate_main(
    module: &mut ObjectModule,
    builder_ctx: &mut FunctionBuilderContext,
    chunk0_id: cranelift_module::FuncId,
    chunks_data_id: cranelift_module::DataId,
    chunks_len_id: cranelift_module::DataId,
) -> Result<(), String> {
    let i64t = types::I64;

    let mut sig = module.make_signature();
    sig.returns.push(AbiParam::new(types::I64)); // exit code

    let main_id = module.declare_function("main", Linkage::Export, &sig)
        .map_err(|e| format!("Failed to declare main: {e}"))?;

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);

    // Declare dgsh_aot_init: (i64, i64) -> i64 (data_ptr, data_len -> context ptr)
    let init_sig = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(i64t)); // chunks data pointer
        s.params.push(AbiParam::new(i64t)); // chunks data length
        s.returns.push(AbiParam::new(i64t));
        s
    };
    let init_id = module.declare_function(H_AOT_INIT, Linkage::Import, &init_sig)
        .map_err(|e| format!("Failed to declare {H_AOT_INIT}: {e}"))?;
    let init_ref = module.declare_func_in_func(init_id, &mut func);

    // Declare dgsh_aot_cleanup: (i64) -> void
    let cleanup_sig = {
        let mut s = module.make_signature();
        s.params.push(AbiParam::new(i64t));
        s
    };
    let cleanup_id = module.declare_function(H_AOT_CLEANUP, Linkage::Import, &cleanup_sig)
        .map_err(|e| format!("Failed to declare {H_AOT_CLEANUP}: {e}"))?;
    let cleanup_ref = module.declare_func_in_func(cleanup_id, &mut func);

    // Declare chunk_0 reference
    let chunk0_ref = module.declare_func_in_func(chunk0_id, &mut func);

    // Declare data references
    let data_gv = module.declare_data_in_func(chunks_data_id, &mut func);
    let len_gv = module.declare_data_in_func(chunks_len_id, &mut func);

    let mut builder = FunctionBuilder::new(&mut func, builder_ctx);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    // Load address of chunks data
    let data_ptr = builder.ins().symbol_value(types::I64, data_gv);
    // Load chunks length (read u64 from the length data)
    let len_ptr = builder.ins().symbol_value(types::I64, len_gv);
    let data_len = builder.ins().load(types::I64, cranelift_codegen::ir::MemFlags::trusted(), len_ptr, 0);

    // 1. Call init with chunks data
    let init_call = builder.ins().call(init_ref, &[data_ptr, data_len]);
    let ctx_ptr = builder.inst_results(init_call)[0];

    // 2. Call chunk_0 (top-level code, no params)
    let _top_call = builder.ins().call(chunk0_ref, &[]);

    // 3. Call cleanup
    builder.ins().call(cleanup_ref, &[ctx_ptr]);

    // 4. Return 0
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().return_(&[zero]);

    builder.finalize();

    let mut ctx = Context::for_function(func);
    module.define_function(main_id, &mut ctx)
        .map_err(|e| format!("Failed to define main: {e}"))?;
    module.clear_context(&mut ctx);

    Ok(())
}

// ---------------------------------------------------------------------------
// Bytecode-to-IR compiler (adapted from GenericJitCompiler in jit.rs)
// ---------------------------------------------------------------------------

/// Emit an inline int fast-path for a binary operation.
fn emit_int_binop(
    builder: &mut FunctionBuilder,
    helpers: &HelperRefs,
    a: cranelift_codegen::ir::Value,
    b: cranelift_codegen::ir::Value,
    fast_op: IntFastOp,
    bop_code: u64,
) -> cranelift_codegen::ir::Value {
    let tag_mask = builder.ins().iconst(types::I64, NB_TAG_MASK as i64);
    let tag_int  = builder.ins().iconst(types::I64, NB_TAG_INT as i64);

    let a_tag    = builder.ins().band(a, tag_mask);
    let b_tag    = builder.ins().band(b, tag_mask);
    let a_is_int = builder.ins().icmp(IntCC::Equal, a_tag, tag_int);
    let b_is_int = builder.ins().icmp(IntCC::Equal, b_tag, tag_int);
    let both_int = builder.ins().band(a_is_int, b_is_int);

    let fast_block  = builder.create_block();
    let slow_block  = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);

    builder.ins().brif(both_int, fast_block, &[], slow_block, &[]);

    // ---- fast path ----
    builder.switch_to_block(fast_block);
    builder.seal_block(fast_block);

    let payload_mask_val = builder.ins().iconst(types::I64, NB_PAYLOAD_MASK as i64);
    let a_payload = builder.ins().band(a, payload_mask_val);
    let b_payload = builder.ins().band(b, payload_mask_val);

    let a_ext = builder.ins().ishl_imm(a_payload, 16);
    let a_ext = builder.ins().sshr_imm(a_ext, 16);
    let b_ext = builder.ins().ishl_imm(b_payload, 16);
    let b_ext = builder.ins().sshr_imm(b_ext, 16);

    let fast_result = match fast_op {
        IntFastOp::Add => {
            let sum = builder.ins().iadd(a_ext, b_ext);
            let masked = builder.ins().band(sum, payload_mask_val);
            builder.ins().bor(masked, tag_int)
        }
        IntFastOp::Sub => {
            let diff = builder.ins().isub(a_ext, b_ext);
            let masked = builder.ins().band(diff, payload_mask_val);
            builder.ins().bor(masked, tag_int)
        }
        IntFastOp::Mul => {
            let prod = builder.ins().imul(a_ext, b_ext);
            let masked = builder.ins().band(prod, payload_mask_val);
            builder.ins().bor(masked, tag_int)
        }
        IntFastOp::Div => {
            let zero = builder.ins().iconst(types::I64, 0);
            let b_is_zero = builder.ins().icmp(IntCC::Equal, b_ext, zero);
            let div_ok_block = builder.create_block();
            builder.ins().brif(b_is_zero, slow_block, &[], div_ok_block, &[]);
            builder.switch_to_block(div_ok_block);
            builder.seal_block(div_ok_block);
            let quot = builder.ins().sdiv(a_ext, b_ext);
            let masked = builder.ins().band(quot, payload_mask_val);
            builder.ins().bor(masked, tag_int)
        }
        IntFastOp::Mod => {
            let zero = builder.ins().iconst(types::I64, 0);
            let b_is_zero = builder.ins().icmp(IntCC::Equal, b_ext, zero);
            let mod_ok_block = builder.create_block();
            builder.ins().brif(b_is_zero, slow_block, &[], mod_ok_block, &[]);
            builder.switch_to_block(mod_ok_block);
            builder.seal_block(mod_ok_block);
            let rem = builder.ins().srem(a_ext, b_ext);
            let masked = builder.ins().band(rem, payload_mask_val);
            builder.ins().bor(masked, tag_int)
        }
        IntFastOp::Eq => {
            let eq = builder.ins().icmp(IntCC::Equal, a, b);
            let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
            let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
            builder.ins().select(eq, true_raw, false_raw)
        }
        IntFastOp::Neq => {
            let ne = builder.ins().icmp(IntCC::NotEqual, a, b);
            let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
            let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
            builder.ins().select(ne, true_raw, false_raw)
        }
        IntFastOp::Lt => {
            let lt = builder.ins().icmp(IntCC::SignedLessThan, a_ext, b_ext);
            let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
            let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
            builder.ins().select(lt, true_raw, false_raw)
        }
        IntFastOp::Gt => {
            let gt = builder.ins().icmp(IntCC::SignedGreaterThan, a_ext, b_ext);
            let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
            let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
            builder.ins().select(gt, true_raw, false_raw)
        }
        IntFastOp::Lte => {
            let le = builder.ins().icmp(IntCC::SignedLessThanOrEqual, a_ext, b_ext);
            let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
            let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
            builder.ins().select(le, true_raw, false_raw)
        }
        IntFastOp::Gte => {
            let ge = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, a_ext, b_ext);
            let true_raw  = builder.ins().iconst(types::I64, (NB_TAG_BOOL | 1) as i64);
            let false_raw = builder.ins().iconst(types::I64, NB_TAG_BOOL as i64);
            builder.ins().select(ge, true_raw, false_raw)
        }
    };
    builder.ins().jump(merge_block, &[fast_result]);

    // ---- slow path ----
    builder.switch_to_block(slow_block);
    builder.seal_block(slow_block);
    let op_c = builder.ins().iconst(types::I64, bop_code as i64);
    let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
    let slow_result = builder.inst_results(call)[0];
    builder.ins().jump(merge_block, &[slow_result]);

    // ---- merge ----
    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    builder.block_params(merge_block)[0]
}

/// Compile a single bytecode chunk to Cranelift IR.
///
/// For chunk 0 (top-level, param_count == 0): no params, no depth check.
/// For function chunks (param_count > 0): params + depth counter, with recursion guard.
fn compile_chunk(
    builder: &mut FunctionBuilder,
    chunk: &Chunk,
    self_ref: cranelift_codegen::ir::FuncRef,
    helpers: &HelperRefs,
    chunk_idx: usize,
    chunk_refs: &HashMap<usize, cranelift_codegen::ir::FuncRef>,
) -> bool {
    let code = &chunk.code;
    let void_raw = Value::void().raw();
    let is_top_level = chunk.param_count == 0;

    // --- Phase 1: Pre-scan for jump targets ---
    let mut jump_targets: HashSet<usize> = HashSet::new();
    {
        let mut pc = 0;
        while pc < code.len() {
            let op: Op = unsafe { std::mem::transmute(code[pc]) };
            pc += 1;
            match op {
                Op::Jump => {
                    let offset = chunk.read_i32(pc);
                    let target = (pc as i32 + 4 + offset) as usize;
                    jump_targets.insert(target);
                    pc += 4;
                }
                Op::JumpIfFalse | Op::JumpIfTrue => {
                    let offset = chunk.read_i32(pc);
                    let target = (pc as i32 + 4 + offset) as usize;
                    jump_targets.insert(target);
                    jump_targets.insert(pc + 4);
                    pc += 4;
                }
                Op::Loop => {
                    let offset = chunk.read_i32(pc);
                    let target = (pc as i32 + 4 - offset) as usize;
                    jump_targets.insert(target);
                    pc += 4;
                }
                Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => {
                    pc += 2 + 8;
                    let offset = chunk.read_i32(pc);
                    let target = (pc as i32 + 4 + offset) as usize;
                    jump_targets.insert(target);
                    jump_targets.insert(pc + 4);
                    pc += 4;
                }
                Op::LoadInt | Op::LoadFloat => pc += 8,
                Op::SubLocalImm | Op::AddLocalImm => pc += 10,
                Op::Call => pc += 4,
                Op::CallLocal | Op::CallBuiltin => pc += 3,
                Op::MakeLambda => pc += 4,
                Op::ErrorField | Op::Alias | Op::Use => pc += 4,
                Op::DefineFunction => pc += 4,
                Op::DefineEnum => {
                    pc += 2;
                    if pc + 1 < code.len() {
                        let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                        pc += 2 + count * 2;
                    }
                }
                Op::TryBegin | Op::TryEnd => pc += 4,
                Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
                | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
                | Op::MakeObject | Op::MakeString | Op::MakeRange
                | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
                | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
                | Op::StringAppendLocal
                | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
                | Op::OptionalCheck | Op::SetErrorTolerant | Op::Import | Op::Free
                | Op::GetLocalInt | Op::RecordError => pc += 2,
                _ => {}
            }
        }
    }

    // --- Phase 2: Create blocks for jump targets ---
    let mut block_map: HashMap<usize, cranelift_codegen::ir::Block> = HashMap::new();
    for &target in &jump_targets {
        block_map.insert(target, builder.create_block());
    }

    // Entry block
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);

    // Declare variables for locals
    let max_locals = chunk.locals.len().max(chunk.param_count as usize).max(16);
    let mut vars: Vec<Variable> = Vec::new();
    for i in 0..max_locals {
        let var = Variable::from_u32(i as u32);
        builder.declare_var(var, types::I64);
        vars.push(var);
    }

    // Depth variable (used for function chunks)
    let depth_var = Variable::from_u32(max_locals as u32);
    builder.declare_var(depth_var, types::I64);

    if is_top_level {
        // Top-level chunk: no params, depth = 0
        let zero = builder.ins().iconst(types::I64, 0);
        builder.def_var(depth_var, zero);
    } else {
        // Function chunk: init params
        for (i, var) in vars.iter().take(chunk.param_count as usize).enumerate() {
            let param_val = builder.block_params(entry)[i];
            builder.def_var(*var, param_val);
        }
        // Depth counter is last param
        let depth_param = builder.block_params(entry)[chunk.param_count as usize];
        builder.def_var(depth_var, depth_param);

        // Depth check at function entry
        let depth_val = builder.use_var(depth_var);
        let limit = builder.ins().iconst(types::I64, 10000);
        let too_deep = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, depth_val, limit);
        let ok_block = builder.create_block();
        let overflow_block = builder.create_block();
        builder.ins().brif(too_deep, overflow_block, &[], ok_block, &[]);

        builder.switch_to_block(overflow_block);
        builder.seal_block(overflow_block);
        builder.ins().call(helpers.recursion_overflow, &[]);
        let void_ret = builder.ins().iconst(types::I64, void_raw as i64);
        builder.ins().return_(&[void_ret]);

        builder.switch_to_block(ok_block);
        builder.seal_block(ok_block);

        // Increment depth
        let one = builder.ins().iconst(types::I64, 1);
        let new_depth = builder.ins().iadd(depth_val, one);
        builder.def_var(depth_var, new_depth);
    }

    // Initialize remaining locals to void
    let void_const = builder.ins().iconst(types::I64, void_raw as i64);
    for var in &vars[chunk.param_count as usize..max_locals] {
        builder.def_var(*var, void_const);
    }

    // Set the current chunk index for constant pool lookups
    // Only set chunk index if this chunk accesses the constant pool
    let needs_chunk_idx = chunk.code.iter().enumerate().any(|(i, &byte)| {
        if i == 0 || i >= chunk.code.len() { return false; }
        let op: Op = unsafe { std::mem::transmute(byte) };
        matches!(op, Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeString
            | Op::Call | Op::MakeObject | Op::MakeLambda | Op::ErrorField
            | Op::GetGlobal | Op::SetGlobal | Op::Import | Op::Free
            | Op::DefineFunction | Op::Alias | Op::Use | Op::OptionalCheck
            | Op::GetDollarField | Op::ErrorCheck | Op::SetErrorTolerant | Op::RecordError)
    });
    if needs_chunk_idx {
        let ci = builder.ins().iconst(types::I64, chunk_idx as i64);
        builder.ins().call(helpers.aot_set_chunk, &[ci]);
    }

    builder.seal_block(entry);

    // Virtual operand stack — uses Cranelift Variables for block-crossing values
    let mut vstack: Vec<cranelift_codegen::ir::Value> = Vec::new();
    // Saved vstack depth for each jump target PC
    let mut saved_vstacks: HashMap<usize, Vec<cranelift_codegen::ir::Value>> = HashMap::new();
    // Persistent stack slots as Cranelift variables (for values that survive block boundaries)
    let max_stack_vars = 16;
    let mut stack_vars: Vec<Variable> = Vec::new();
    let next_var_base = if chunk.param_count > 0 { chunk.param_count as usize + 1 + chunk.locals.len() + 10 } else { chunk.locals.len() + 10 };
    for i in 0..max_stack_vars {
        let v = Variable::from_u32((next_var_base + 100 + i) as u32);
        builder.declare_var(v, types::I64);
        let init_val = builder.ins().iconst(types::I64, void_raw as i64);
        builder.def_var(v, init_val);
        stack_vars.push(v);
    }

    // --- Phase 3: Translate opcodes ---
    let mut pc = 0;
    let block_sealed: HashSet<cranelift_codegen::ir::Block> = HashSet::new();
    let mut terminated = false;

    while pc < code.len() {
        // Check if this PC is a jump target
        if let Some(&block) = block_map.get(&pc) {
            if !terminated {
                // Save vstack to Cranelift variables before jumping
                for (i, &val) in vstack.iter().enumerate() {
                    if i < max_stack_vars {
                        builder.def_var(stack_vars[i], val);
                    }
                }
                builder.ins().jump(block, &[]);
            }
            builder.switch_to_block(block);
            // Restore vstack from Cranelift variables using saved depth
            if let Some(saved) = saved_vstacks.get(&pc) {
                let depth = saved.len();
                vstack.clear();
                for i in 0..depth.min(max_stack_vars) {
                    vstack.push(builder.use_var(stack_vars[i]));
                }
            } else {
                // No explicit save — keep vstack empty (unreachable from linear flow)
                vstack.clear();
            }
            terminated = false;
        }

        let op: Op = unsafe { std::mem::transmute(code[pc]) };
        pc += 1;

        if terminated {
            // Skip dead code after terminator until next block
            match op {
                Op::LoadInt | Op::LoadFloat => pc += 8,
                Op::SubLocalImm | Op::AddLocalImm => pc += 10,
                Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => pc += 14,
                Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue | Op::Loop
                | Op::TryBegin | Op::TryEnd => pc += 4,
                Op::Call => pc += 4,
                Op::CallLocal | Op::CallBuiltin => pc += 3,
                Op::MakeLambda => pc += 4,
                Op::ErrorField | Op::Alias | Op::Use | Op::DefineFunction => pc += 4,
                Op::DefineEnum => {
                    pc += 2;
                    if pc + 1 < code.len() {
                        let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                        pc += 2 + count * 2;
                    }
                }
                Op::GetLocal | Op::SetLocal | Op::GetGlobal | Op::SetGlobal
                | Op::LoadConst | Op::FieldGet | Op::FieldSet | Op::MakeList
                | Op::MakeObject | Op::MakeString | Op::MakeRange
                | Op::IncLocal | Op::DecLocal | Op::PostIncLocal | Op::PostDecLocal
                | Op::PreIncLocal | Op::PreDecLocal | Op::CompoundAddInt | Op::CompoundSubInt
                | Op::StringAppendLocal
                | Op::GetDollarIndex | Op::GetDollarField | Op::ErrorCheck
                | Op::OptionalCheck | Op::SetErrorTolerant | Op::Import | Op::Free
                | Op::GetLocalInt | Op::RecordError => pc += 2,
                _ => {}
            }
            continue;
        }

        match op {
            // ============================================================
            // CONSTANTS
            // ============================================================
            Op::LoadInt => {
                let val = chunk.read_i64(pc); pc += 8;
                let nan_boxed = Value::int(val);
                let raw = nan_boxed.raw();
                std::mem::forget(nan_boxed);
                vstack.push(builder.ins().iconst(types::I64, raw as i64));
            }
            Op::LoadFloat => {
                let bits = chunk.read_i64(pc); pc += 8;
                vstack.push(builder.ins().iconst(types::I64, bits));
            }
            Op::LoadTrue => {
                let raw = Value::bool(true).raw();
                vstack.push(builder.ins().iconst(types::I64, raw as i64));
            }
            Op::LoadFalse => {
                let raw = Value::bool(false).raw();
                vstack.push(builder.ins().iconst(types::I64, raw as i64));
            }
            Op::LoadVoid => {
                vstack.push(builder.ins().iconst(types::I64, void_raw as i64));
            }
            Op::LoadConst => {
                let idx = chunk.read_u16(pc) as usize; pc += 2;
                let chunk_idx_val = builder.ins().iconst(types::I64, chunk_idx as i64);
                let const_idx_val = builder.ins().iconst(types::I64, idx as i64);
                let call = builder.ins().call(helpers.aot_load_const, &[chunk_idx_val, const_idx_val]);
                vstack.push(builder.inst_results(call)[0]);
            }

            // ============================================================
            // VARIABLES
            // ============================================================
            Op::GetLocal | Op::GetLocalInt => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let call = builder.ins().call(helpers.clone_value, &[v]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::SetLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let val = match vstack.pop() { Some(v) => v, None => return false };
                if slot >= vars.len() { return false; }
                builder.def_var(vars[slot], val);
            }
            Op::GetGlobal => {
                let idx = chunk.read_u16(pc) as u64; pc += 2;
                let idx_val = builder.ins().iconst(types::I64, idx as i64);
                let call = builder.ins().call(helpers.get_global, &[idx_val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::SetGlobal => {
                let idx = chunk.read_u16(pc) as u64; pc += 2;
                let val = match vstack.pop() { Some(v) => v, None => return false };
                let idx_val = builder.ins().iconst(types::I64, idx as i64);
                builder.ins().call(helpers.set_global, &[idx_val, val]);
            }

            // ============================================================
            // ARITHMETIC
            // ============================================================
            Op::Add | Op::AddInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Add, BOP_ADD));
            }
            Op::Sub | Op::SubInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Sub, BOP_SUB));
            }
            Op::Mul | Op::MulInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Mul, BOP_MUL));
            }
            Op::Div | Op::DivInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Div, BOP_DIV));
            }
            Op::Mod | Op::ModInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Mod, BOP_MOD));
            }
            Op::Pow => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.pow, &[a, b]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::Neg | Op::NegInt => {
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.negate, &[a]);
                vstack.push(builder.inst_results(call)[0]);
            }

            // ============================================================
            // COMPARISON
            // ============================================================
            Op::Eq | Op::EqInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Eq, BOP_EQ));
            }
            Op::Neq | Op::NeqInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Neq, BOP_NEQ));
            }
            Op::Lt | Op::LtInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Lt, BOP_LT));
            }
            Op::Gt | Op::GtInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Gt, BOP_GT));
            }
            Op::Lte | Op::LteInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Lte, BOP_LTE));
            }
            Op::Gte | Op::GteInt => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                vstack.push(emit_int_binop(builder, helpers, a, b, IntFastOp::Gte, BOP_GTE));
            }

            // ============================================================
            // LOGICAL / BITWISE
            // ============================================================
            Op::Not => {
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.not, &[a]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::BitAnd => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let op_c = builder.ins().iconst(types::I64, BOP_BITAND as i64);
                let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::BitOr => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let op_c = builder.ins().iconst(types::I64, BOP_BITOR as i64);
                let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::BitXor => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let op_c = builder.ins().iconst(types::I64, BOP_BITXOR as i64);
                let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::BitNot => {
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.bitnot, &[a]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::Shl => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let op_c = builder.ins().iconst(types::I64, BOP_SHL as i64);
                let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::Shr => {
                let b = match vstack.pop() { Some(v) => v, None => return false };
                let a = match vstack.pop() { Some(v) => v, None => return false };
                let op_c = builder.ins().iconst(types::I64, BOP_SHR as i64);
                let call = builder.ins().call(helpers.binary_op, &[a, b, op_c]);
                vstack.push(builder.inst_results(call)[0]);
            }

            // ============================================================
            // INC/DEC
            // ============================================================
            Op::IncLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let op_c = builder.ins().iconst(types::I64, 0);
                let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
            }
            Op::DecLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let op_c = builder.ins().iconst(types::I64, 1);
                let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
            }
            Op::PostIncLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let old = builder.use_var(vars[slot]);
                let old_clone = builder.ins().call(helpers.clone_value, &[old]);
                let old_cloned = builder.inst_results(old_clone)[0];
                let op_c = builder.ins().iconst(types::I64, 0);
                let call = builder.ins().call(helpers.inc_dec, &[old, op_c]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
                vstack.push(old_cloned);
            }
            Op::PostDecLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let old = builder.use_var(vars[slot]);
                let old_clone = builder.ins().call(helpers.clone_value, &[old]);
                let old_cloned = builder.inst_results(old_clone)[0];
                let op_c = builder.ins().iconst(types::I64, 1);
                let call = builder.ins().call(helpers.inc_dec, &[old, op_c]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
                vstack.push(old_cloned);
            }
            Op::PreIncLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let op_c = builder.ins().iconst(types::I64, 0);
                let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                let new_v = builder.inst_results(call)[0];
                builder.def_var(vars[slot], new_v);
                let clone_call = builder.ins().call(helpers.clone_value, &[new_v]);
                vstack.push(builder.inst_results(clone_call)[0]);
            }
            Op::PreDecLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let op_c = builder.ins().iconst(types::I64, 1);
                let call = builder.ins().call(helpers.inc_dec, &[v, op_c]);
                let new_v = builder.inst_results(call)[0];
                builder.def_var(vars[slot], new_v);
                let clone_call = builder.ins().call(helpers.clone_value, &[new_v]);
                vstack.push(builder.inst_results(clone_call)[0]);
            }
            Op::CompoundAddInt => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let rhs = match vstack.pop() { Some(v) => v, None => return false };
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let op_c = builder.ins().iconst(types::I64, 0);
                let call = builder.ins().call(helpers.compound_op, &[v, rhs, op_c]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
            }
            Op::CompoundSubInt => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let rhs = match vstack.pop() { Some(v) => v, None => return false };
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let op_c = builder.ins().iconst(types::I64, 1);
                let call = builder.ins().call(helpers.compound_op, &[v, rhs, op_c]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
            }
            Op::StringAppendLocal => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let rhs = match vstack.pop() { Some(v) => v, None => return false };
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let call = builder.ins().call(helpers.string_append_local, &[v, rhs]);
                builder.def_var(vars[slot], builder.inst_results(call)[0]);
            }

            // ============================================================
            // SUPERINSTRUCTIONS
            // ============================================================
            Op::SubLocalImm => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let imm = chunk.read_i64(pc); pc += 8;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                let sign_ext = builder.ins().ishl_imm(payload, 16);
                let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                let result = builder.ins().iadd_imm(sign_ext, -imm);
                let masked = builder.ins().band_imm(result, NB_PAYLOAD_MASK as i64);
                let tagged = builder.ins().bor_imm(masked, NB_TAG_INT as i64);
                vstack.push(tagged);
            }
            Op::AddLocalImm => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let imm = chunk.read_i64(pc); pc += 8;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                let sign_ext = builder.ins().ishl_imm(payload, 16);
                let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                let result = builder.ins().iadd_imm(sign_ext, imm);
                let masked = builder.ins().band_imm(result, NB_PAYLOAD_MASK as i64);
                let tagged = builder.ins().bor_imm(masked, NB_TAG_INT as i64);
                vstack.push(tagged);
            }
            Op::BranchIfLocalGtImm => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let imm = chunk.read_i64(pc); pc += 8;
                let offset = chunk.read_i32(pc); pc += 4;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                let sign_ext = builder.ins().ishl_imm(payload, 16);
                let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                let imm_val = builder.ins().iconst(types::I64, imm);
                let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, sign_ext, imm_val);
                let target_pc = (pc as i32 + offset) as usize;
                let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                terminated = true;
            }
            Op::BranchIfLocalLteImm => {
                let slot = chunk.read_u16(pc) as usize; pc += 2;
                let imm = chunk.read_i64(pc); pc += 8;
                let offset = chunk.read_i32(pc); pc += 4;
                if slot >= vars.len() { return false; }
                let v = builder.use_var(vars[slot]);
                let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                let sign_ext = builder.ins().ishl_imm(payload, 16);
                let sign_ext = builder.ins().sshr_imm(sign_ext, 16);
                let imm_val = builder.ins().iconst(types::I64, imm);
                let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, sign_ext, imm_val);
                let target_pc = (pc as i32 + offset) as usize;
                let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                terminated = true;
            }

            // ============================================================
            // CONTROL FLOW
            // ============================================================
            Op::Jump => {
                let offset = chunk.read_i32(pc); pc += 4;
                let target_pc = (pc as i32 + offset) as usize;
                if let Some(&block) = block_map.get(&target_pc) {
                    for (i, &val) in vstack.iter().enumerate() {
                        if i < max_stack_vars {
                            builder.def_var(stack_vars[i], val);
                        }
                    }
                    saved_vstacks.entry(target_pc).or_insert_with(|| vstack.clone());
                    builder.ins().jump(block, &[]);
                    terminated = true;
                } else {
                    
                    
                    return false;
                }
            }
            Op::JumpIfFalse => {
                let offset = chunk.read_i32(pc); pc += 4;
                let cond = match vstack.pop() { Some(v) => v, None => { eprintln!("AOT: JumpIfFalse vstack empty at pc={}", pc-4); return false; } };
                let target_pc = (pc as i32 + offset) as usize;
                let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => { eprintln!("AOT: JumpIfFalse target {target_pc} not in block_map at pc={}", pc-4); return false; } };
                let fall_block = match block_map.get(&pc) { Some(&b) => b, None => { eprintln!("AOT: JumpIfFalse fall-through {pc} not in block_map at pc={}", pc-4); return false; } };
                // Save vstack depth + variables for both branch targets
                for (i, &val) in vstack.iter().enumerate() {
                    if i < max_stack_vars {
                        builder.def_var(stack_vars[i], val);
                    }
                }
                saved_vstacks.insert(target_pc, vstack.clone());
                saved_vstacks.insert(pc, vstack.clone());
                let call = builder.ins().call(helpers.is_truthy, &[cond]);
                let truthy = builder.inst_results(call)[0];
                let z = builder.ins().iconst(types::I64, 0);
                let cmp = builder.ins().icmp(IntCC::Equal, truthy, z);
                builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                terminated = true;
            }
            Op::JumpIfTrue => {
                let offset = chunk.read_i32(pc); pc += 4;
                let cond = match vstack.pop() { Some(v) => v, None => return false };
                let target_pc = (pc as i32 + offset) as usize;
                let target_block = match block_map.get(&target_pc) { Some(&b) => b, None => return false };
                let fall_block = match block_map.get(&pc) { Some(&b) => b, None => return false };
                let call = builder.ins().call(helpers.is_truthy, &[cond]);
                let truthy = builder.inst_results(call)[0];
                let z = builder.ins().iconst(types::I64, 0);
                let cmp = builder.ins().icmp(IntCC::NotEqual, truthy, z);
                builder.ins().brif(cmp, target_block, &[], fall_block, &[]);
                terminated = true;
            }
            Op::Loop => {
                let offset = chunk.read_i32(pc); pc += 4;
                let target_pc = (pc as i32 - offset) as usize;
                if let Some(&block) = block_map.get(&target_pc) {
                    builder.ins().jump(block, &[]);
                    terminated = true;
                } else {
                    eprintln!("AOT: Loop target {target_pc} not in block_map at pc={}", pc-4);
                    return false;
                }
            }

            // ============================================================
            // CALLS
            // ============================================================
            Op::CallLocal => {
                let target = chunk.read_u16(pc) as usize; pc += 2;
                let argc = code[pc] as usize; pc += 1;
                let mut args = Vec::new();
                for _ in 0..argc {
                    match vstack.pop() { Some(v) => args.push(v), None => return false }
                }
                args.reverse();
                if target == chunk_idx {
                    // Self-recursive call
                    let cur_depth = builder.use_var(depth_var);
                    args.push(cur_depth);
                    let call = builder.ins().call(self_ref, &args);
                    vstack.push(builder.inst_results(call)[0]);
                    if needs_chunk_idx {
                        let ci = builder.ins().iconst(types::I64, chunk_idx as i64);
                        builder.ins().call(helpers.aot_set_chunk, &[ci]);
                    }
                } else if let Some(&target_ref) = chunk_refs.get(&target) {
                    // Direct call to another AOT-compiled function
                    let zero_depth = builder.ins().iconst(types::I64, 0);
                    args.push(zero_depth);
                    let call = builder.ins().call(target_ref, &args);
                    vstack.push(builder.inst_results(call)[0]);
                    if needs_chunk_idx {
                        let ci = builder.ins().iconst(types::I64, chunk_idx as i64);
                        builder.ins().call(helpers.aot_set_chunk, &[ci]);
                    }
                } else {
                    // Fallback to VM call (should not happen in full AOT)
                    let target_val = builder.ins().iconst(types::I64, target as i64);
                    if argc == 0 {
                        let null_ptr = builder.ins().iconst(types::I64, 0);
                        let argc_val = builder.ins().iconst(types::I64, 0);
                        let call = builder.ins().call(helpers.vm_call, &[target_val, null_ptr, argc_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (argc * 8) as u32, 3));
                        for (i, val) in args.iter().enumerate() {
                            builder.ins().stack_store(*val, slot, (i * 8) as i32);
                        }
                        let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                        let argc_val = builder.ins().iconst(types::I64, argc as i64);
                        let call = builder.ins().call(helpers.vm_call, &[target_val, ptr, argc_val]);
                        vstack.push(builder.inst_results(call)[0]);
                    }
                }
            }
            Op::Call => {
                let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                let argc = code[pc] as usize; pc += 1;
                let res = code[pc] as u64; pc += 1;
                let mut args = Vec::new();
                for _ in 0..argc {
                    match vstack.pop() { Some(v) => args.push(v), None => return false }
                }
                args.reverse();
                if argc == 0 {
                    let null_ptr = builder.ins().iconst(types::I64, 0);
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let res_val = builder.ins().iconst(types::I64, res as i64);
                    let argc_val = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.generic_call, &[name_idx_val, res_val, null_ptr, argc_val]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (argc * 8) as u32, 3));
                    for (i, val) in args.iter().enumerate() {
                        builder.ins().stack_store(*val, slot, (i * 8) as i32);
                    }
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let res_val = builder.ins().iconst(types::I64, res as i64);
                    let argc_val = builder.ins().iconst(types::I64, argc as i64);
                    let call = builder.ins().call(helpers.generic_call, &[name_idx_val, res_val, ptr, argc_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
            }
            Op::CallBuiltin => {
                let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                let argc = code[pc] as usize; pc += 1;
                let mut args = Vec::new();
                for _ in 0..argc {
                    match vstack.pop() { Some(v) => args.push(v), None => return false }
                }
                args.reverse();
                if argc == 0 {
                    let null_ptr = builder.ins().iconst(types::I64, 0);
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let argc_val = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.call_builtin, &[name_idx_val, null_ptr, argc_val]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (argc * 8) as u32, 3));
                    for (i, val) in args.iter().enumerate() {
                        builder.ins().stack_store(*val, slot, (i * 8) as i32);
                    }
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let argc_val = builder.ins().iconst(types::I64, argc as i64);
                    let call = builder.ins().call(helpers.call_builtin, &[name_idx_val, ptr, argc_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
            }
            Op::Return => {
                let val = match vstack.pop() { Some(v) => v, None => return false };
                builder.ins().return_(&[val]);
                terminated = true;
            }
            Op::ReturnVoid => {
                let z = builder.ins().iconst(types::I64, void_raw as i64);
                builder.ins().return_(&[z]);
                terminated = true;
            }

            // ============================================================
            // STACK OPS
            // ============================================================
            Op::Pop => { vstack.pop(); }
            Op::Dup => {
                // If vstack is empty but we're at a merge point after a branch,
                // try to restore from stack variables
                if vstack.is_empty() {
                    // Look backwards in saved_vstacks for the nearest saved state
                    for back_pc in (0..pc).rev() {
                        if let Some(saved) = saved_vstacks.get(&back_pc) {
                            if !saved.is_empty() {
                                // Restore the depth from the nearest saved state
                                for i in 0..saved.len().min(max_stack_vars) {
                                    vstack.push(builder.use_var(stack_vars[i]));
                                }
                                break;
                            }
                        }
                    }
                }
                if let Some(&v) = vstack.last() {
                    let call = builder.ins().call(helpers.clone_value, &[v]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    eprintln!("AOT: Dup vstack empty at pc={}", pc-1);
                    return false;
                }
            }
            Op::CheckCancel => {}

            // ============================================================
            // COLLECTIONS
            // ============================================================
            Op::MakeList => {
                let count = chunk.read_u16(pc) as usize; pc += 2;
                if count == 0 {
                    let null_ptr = builder.ins().iconst(types::I64, 0);
                    let count_val = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.make_list, &[null_ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (count * 8) as u32, 3));
                    let mut items = Vec::with_capacity(count);
                    for _ in 0..count {
                        match vstack.pop() { Some(v) => items.push(v), None => return false }
                    }
                    items.reverse();
                    for (i, val) in items.iter().enumerate() {
                        builder.ins().stack_store(*val, slot, (i * 8) as i32);
                    }
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    let count_val = builder.ins().iconst(types::I64, count as i64);
                    let call = builder.ins().call(helpers.make_list, &[ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
            }
            Op::MakeObject => {
                let count = chunk.read_u16(pc) as usize; pc += 2;
                if count == 0 {
                    let null_ptr = builder.ins().iconst(types::I64, 0);
                    let count_val = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.make_object, &[null_ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (count * 2 * 8) as u32, 3));
                    let mut pairs = Vec::with_capacity(count);
                    for _ in 0..count {
                        let val = match vstack.pop() { Some(v) => v, None => return false };
                        let key = match vstack.pop() { Some(v) => v, None => return false };
                        pairs.push((key, val));
                    }
                    pairs.reverse();
                    for (i, (key, val)) in pairs.iter().enumerate() {
                        builder.ins().stack_store(*key, slot, (i * 2 * 8) as i32);
                        builder.ins().stack_store(*val, slot, (i * 2 * 8 + 8) as i32);
                    }
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    let count_val = builder.ins().iconst(types::I64, count as i64);
                    let call = builder.ins().call(helpers.make_object, &[ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
            }
            Op::MakeString => {
                let count = chunk.read_u16(pc) as usize; pc += 2;
                if count == 0 {
                    let null_ptr = builder.ins().iconst(types::I64, 0);
                    let count_val = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.make_string, &[null_ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (count * 8) as u32, 3));
                    let mut parts = Vec::with_capacity(count);
                    for _ in 0..count {
                        match vstack.pop() { Some(v) => parts.push(v), None => return false }
                    }
                    parts.reverse();
                    for (i, val) in parts.iter().enumerate() {
                        builder.ins().stack_store(*val, slot, (i * 8) as i32);
                    }
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    let count_val = builder.ins().iconst(types::I64, count as i64);
                    let call = builder.ins().call(helpers.make_string, &[ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
            }
            Op::MakeRange => {
                let end_v = match vstack.pop() { Some(v) => v, None => return false };
                let start_v = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.make_range, &[start_v, end_v]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::Index => {
                let idx = match vstack.pop() { Some(v) => v, None => return false };
                let target = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.index_get, &[target, idx]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::IndexSet => {
                let val = match vstack.pop() { Some(v) => v, None => return false };
                let idx = match vstack.pop() { Some(v) => v, None => return false };
                let target = match vstack.pop() { Some(v) => v, None => return false };
                builder.ins().call(helpers.index_set, &[target, idx, val]);
            }
            Op::FieldGet => {
                let field_idx = chunk.read_u16(pc) as u64; pc += 2;
                let obj = match vstack.pop() { Some(v) => v, None => return false };
                let field_idx_val = builder.ins().iconst(types::I64, field_idx as i64);
                let call = builder.ins().call(helpers.field_get, &[obj, field_idx_val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::FieldSet => {
                let field_idx = chunk.read_u16(pc) as u64; pc += 2;
                let val = match vstack.pop() { Some(v) => v, None => return false };
                let obj = match vstack.pop() { Some(v) => v, None => return false };
                let field_idx_val = builder.ins().iconst(types::I64, field_idx as i64);
                builder.ins().call(helpers.field_set, &[obj, field_idx_val, val]);
            }

            // ============================================================
            // FUNCTION/MODULE DEFINITIONS
            // ============================================================
            Op::DefineFunction => {
                let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                let fn_chunk = chunk.read_u16(pc) as u64; pc += 2;
                let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                let fn_chunk_val = builder.ins().iconst(types::I64, fn_chunk as i64);
                builder.ins().call(helpers.define_function, &[name_idx_val, fn_chunk_val]);
            }
            Op::DefineEnum => {
                pc += 2;
                if pc + 1 < code.len() {
                    let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                    pc += 2 + count * 2;
                }
                // DefineEnum is complex; skip for AOT (enums still work via VM fallback)
            }
            Op::Import => {
                let _path_idx = chunk.read_u16(pc); pc += 2;
                // Imports resolved at compile time in AOT
            }
            Op::Free => {
                let idx = chunk.read_u16(pc) as u64; pc += 2;
                let idx_val = builder.ins().iconst(types::I64, idx as i64);
                builder.ins().call(helpers.free_global, &[idx_val]);
            }
            Op::Alias => {
                let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                let target_idx = chunk.read_u16(pc) as u64; pc += 2;
                let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                let target_idx_val = builder.ins().iconst(types::I64, target_idx as i64);
                builder.ins().call(helpers.alias, &[name_idx_val, target_idx_val]);
            }
            Op::Use => {
                let path_idx = chunk.read_u16(pc) as u64; pc += 2;
                let alias_idx = chunk.read_u16(pc) as u64; pc += 2;
                let path_idx_val = builder.ins().iconst(types::I64, path_idx as i64);
                let alias_idx_val = builder.ins().iconst(types::I64, alias_idx as i64);
                builder.ins().call(helpers.use_fn, &[path_idx_val, alias_idx_val]);
            }
            Op::Throw => {
                let val = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.throw, &[val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::TryBegin => {
                // For AOT, we skip try/catch for now (same as JIT)
                let _offset = chunk.read_i32(pc); pc += 4;
            }
            Op::TryEnd => {
                let _offset = chunk.read_i32(pc); pc += 4;
            }
            Op::MakeLambda => {
                let name_idx = chunk.read_u16(pc) as u64; pc += 2;
                let res = code[pc] as u64; pc += 1;
                let bound_count = code[pc] as usize; pc += 1;
                let mut bound_args = Vec::new();
                for _ in 0..bound_count {
                    match vstack.pop() { Some(v) => bound_args.push(v), None => return false }
                }
                bound_args.reverse();
                if bound_count == 0 {
                    let null_ptr = builder.ins().iconst(types::I64, 0);
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let res_val = builder.ins().iconst(types::I64, res as i64);
                    let count_val = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(helpers.make_lambda, &[name_idx_val, res_val, null_ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                } else {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (bound_count * 8) as u32, 3));
                    for (i, val) in bound_args.iter().enumerate() {
                        builder.ins().stack_store(*val, slot, (i * 8) as i32);
                    }
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    let name_idx_val = builder.ins().iconst(types::I64, name_idx as i64);
                    let res_val = builder.ins().iconst(types::I64, res as i64);
                    let count_val = builder.ins().iconst(types::I64, bound_count as i64);
                    let call = builder.ins().call(helpers.make_lambda, &[name_idx_val, res_val, ptr, count_val]);
                    vstack.push(builder.inst_results(call)[0]);
                }
            }
            Op::ErrorCheck => {
                let slot = chunk.read_u16(pc) as u64; pc += 2;
                let slot_val = builder.ins().iconst(types::I64, slot as i64);
                let call = builder.ins().call(helpers.error_check, &[slot_val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::ErrorField => {
                let slot = chunk.read_u16(pc) as u64; pc += 2;
                let _field_idx = chunk.read_u16(pc); pc += 2;
                let slot_val = builder.ins().iconst(types::I64, slot as i64);
                let call = builder.ins().call(helpers.error_field, &[slot_val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::SetErrorTolerant => {
                let slot = chunk.read_u16(pc) as u64; pc += 2;
                let slot_val = builder.ins().iconst(types::I64, slot as i64);
                builder.ins().call(helpers.set_error_tolerant, &[slot_val]);
            }
            Op::RecordError => {
                let slot = chunk.read_u16(pc) as u64; pc += 2;
                let slot_val = builder.ins().iconst(types::I64, slot as i64);
                builder.ins().call(helpers.record_error, &[slot_val]);
            }
            Op::OptionalCheck => {
                let slot = chunk.read_u16(pc) as u64; pc += 2;
                let slot_val = builder.ins().iconst(types::I64, slot as i64);
                let call = builder.ins().call(helpers.optional_check, &[slot_val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::Atomic => {
                let val = match vstack.pop() { Some(v) => v, None => return false };
                let call = builder.ins().call(helpers.atomic, &[val]);
                vstack.push(builder.inst_results(call)[0]);
            }

            // ============================================================
            // SEND CONTEXT
            // ============================================================
            Op::PushSendCtx => {
                let val = match vstack.pop() { Some(v) => v, None => return false };
                builder.ins().call(helpers.push_send_ctx, &[val]);
            }
            Op::PopSendCtx => {
                builder.ins().call(helpers.pop_send_ctx, &[]);
            }
            Op::GetDollar => {
                let call = builder.ins().call(helpers.get_dollar, &[]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::GetDollarIndex => {
                let idx = chunk.read_u16(pc) as u64; pc += 2;
                let idx_val = builder.ins().iconst(types::I64, idx as i64);
                let call = builder.ins().call(helpers.get_dollar_index, &[idx_val]);
                vstack.push(builder.inst_results(call)[0]);
            }
            Op::GetDollarField => {
                let field_idx = chunk.read_u16(pc) as u64; pc += 2;
                let field_idx_val = builder.ins().iconst(types::I64, field_idx as i64);
                let call = builder.ins().call(helpers.get_dollar_field, &[field_idx_val]);
                vstack.push(builder.inst_results(call)[0]);
            }

            // ============================================================
            // SCOPE (no-op)
            // ============================================================
            Op::PushScope | Op::PopScope => {}

            Op::IntToFloat => {
                if let Some(v) = vstack.last().copied() {
                    let tag = builder.ins().band_imm(v, NB_TAG_MASK as i64);
                    let tag_int = builder.ins().iconst(types::I64, NB_TAG_INT as i64);
                    let is_int = builder.ins().icmp(IntCC::Equal, tag, tag_int);

                    let convert_block = builder.create_block();
                    let skip_block = builder.create_block();
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    builder.ins().brif(is_int, convert_block, &[], skip_block, &[]);

                    builder.switch_to_block(convert_block);
                    builder.seal_block(convert_block);
                    let payload = builder.ins().band_imm(v, NB_PAYLOAD_MASK as i64);
                    let sign_extended = builder.ins().ishl_imm(payload, 16);
                    let sign_extended = builder.ins().sshr_imm(sign_extended, 16);
                    let as_float = builder.ins().fcvt_from_sint(types::F64, sign_extended);
                    let float_bits = builder.ins().bitcast(types::I64, cranelift_codegen::ir::MemFlags::new(), as_float);
                    builder.ins().jump(merge_block, &[float_bits]);

                    builder.switch_to_block(skip_block);
                    builder.seal_block(skip_block);
                    builder.ins().jump(merge_block, &[v]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let result = builder.block_params(merge_block)[0];
                    *vstack.last_mut().expect("checked above") = result;
                }
            }
        }
    }

    // If we reach the end without a terminator
    if !terminated {
        if let Some(val) = vstack.pop() {
            builder.ins().return_(&[val]);
        } else {
            let z = builder.ins().iconst(types::I64, void_raw as i64);
            builder.ins().return_(&[z]);
        }
    }

    // Seal all blocks
    for &block in block_map.values() {
        if !block_sealed.contains(&block) {
            builder.seal_block(block);
        }
    }

    true
}
