use super::bytecode::{Chunk, Op};

/// Analyze a function chunk and decide: should it be JIT'd or stay in TreeWalk/VM?
/// Returns true if JIT is recommended.
pub fn should_jit(chunk: &Chunk) -> bool {
    let (jit_score, tw_score) = score_chunk(chunk);
    // JIT wins if it scores significantly higher
    jit_score > 0 && jit_score as f64 > tw_score as f64 * 1.5
}

/// Analyze a set of chunks and decide the best execution mode.
/// Returns Jit if any function benefits from JIT, otherwise TreeWalk.
pub fn choose_mode(chunks: &[Chunk]) -> super::ExecutionMode {
    let mut any_jit = false;
    let mut all_tw = true;

    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 { continue; } // skip top-level chunk
        if chunk.param_count == 0 { continue; } // skip non-functions
        if should_jit(chunk) {
            any_jit = true;
            all_tw = false;
        }
    }

    if any_jit {
        super::ExecutionMode::Jit
    } else if all_tw {
        super::ExecutionMode::TreeWalk
    } else {
        super::ExecutionMode::Vm
    }
}

/// Score a chunk for JIT suitability vs TreeWalk suitability.
/// Returns (jit_score, treewalk_score).
fn score_chunk(chunk: &Chunk) -> (u32, u32) {
    let mut jit: u32 = 0;
    let mut tw: u32 = 0;
    let code = &chunk.code;
    let mut pc = 0;

    while pc < code.len() {
        let op: Op = unsafe { std::mem::transmute(code[pc]) };
        pc += 1;

        match op {
            // --- Strong JIT indicators ---
            // Int arithmetic: native i64 ops, massive speedup from JIT
            Op::AddInt | Op::SubInt | Op::MulInt | Op::DivInt | Op::ModInt | Op::NegInt => { jit += 3; }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod => { jit += 2; }
            Op::Pow => { jit += 1; }

            // Int comparisons
            Op::EqInt | Op::NeqInt | Op::LtInt | Op::GtInt | Op::LteInt | Op::GteInt => { jit += 2; }
            Op::Eq | Op::Neq | Op::Lt | Op::Gt | Op::Lte | Op::Gte => { jit += 1; }

            // Recursive calls: huge JIT win
            Op::CallLocal => { jit += 5; pc += 3; continue; }

            // Superinstructions: already optimized for int, JIT makes them native
            Op::SubLocalImm | Op::AddLocalImm => { jit += 4; pc += 10; continue; }
            Op::BranchIfLocalGtImm | Op::BranchIfLocalLteImm => { jit += 4; pc += 14; continue; }

            // Local variable access: stack slots, fast in both but JIT uses registers
            Op::GetLocal | Op::SetLocal => { jit += 1; pc += 2; continue; }

            // Inc/Dec: tight loop patterns
            Op::IncLocal | Op::DecLocal | Op::CompoundAddInt | Op::CompoundSubInt | Op::StringAppendLocal => { jit += 2; pc += 2; continue; }
            Op::PostIncLocal | Op::PostDecLocal | Op::PreIncLocal | Op::PreDecLocal => { jit += 2; pc += 2; continue; }

            // Bitwise: native ops
            Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitNot | Op::Shl | Op::Shr => { jit += 2; }

            // Control flow: JIT handles branches natively
            Op::Jump => { jit += 1; pc += 4; continue; }
            Op::JumpIfFalse | Op::JumpIfTrue => { jit += 1; pc += 4; continue; }
            Op::Loop => { jit += 2; pc += 4; continue; } // loops = big JIT win

            // Constants
            Op::LoadInt => { jit += 1; pc += 8; continue; }
            Op::LoadTrue | Op::LoadFalse | Op::LoadVoid => { jit += 1; }
            Op::Not | Op::Neg => { jit += 1; }

            // --- Strong TreeWalk indicators ---
            // String operations: heap allocation, no JIT benefit
            Op::LoadConst => { tw += 2; pc += 2; continue; }
            Op::MakeString => { tw += 3; pc += 2; continue; }

            // Object/List creation: heap, complex
            Op::MakeList => { tw += 2; pc += 2; continue; }
            Op::MakeObject => { tw += 3; pc += 2; continue; }

            // Field/Index access on objects: dynamic dispatch
            Op::FieldGet | Op::FieldSet => { tw += 2; pc += 2; continue; }
            Op::Index | Op::IndexSet => { tw += 2; }

            // Generic calls (builtins, executables): interpreter overhead
            Op::Call => { tw += 3; pc += 4; continue; }
            Op::CallBuiltin => { tw += 3; pc += 3; continue; }

            // Send operator: complex state management
            Op::PushSendCtx | Op::PopSendCtx => { tw += 3; }
            Op::GetDollar => { tw += 2; }
            Op::GetDollarIndex => { tw += 2; pc += 2; continue; }
            Op::GetDollarField => { tw += 2; pc += 2; continue; }

            // Error handling: complex control flow
            Op::TryBegin | Op::TryEnd => { tw += 3; pc += 4; continue; }
            Op::ErrorCheck | Op::SetErrorTolerant | Op::RecordError | Op::OptionalCheck => { tw += 2; pc += 2; continue; }
            Op::ErrorField => { tw += 2; pc += 4; continue; }
            Op::Throw => { tw += 3; }

            // Lambda: dynamic dispatch
            Op::MakeLambda => { tw += 2; pc += 4; continue; }

            // Global access: HashMap lookup, slower than locals
            Op::GetGlobal | Op::SetGlobal => { tw += 1; pc += 2; continue; }

            // Import/Use/Alias: pure interpreter features
            Op::Import | Op::Free => { tw += 3; pc += 2; continue; }
            Op::Use | Op::Alias => { tw += 3; pc += 4; continue; }
            Op::DefineEnum => {
                tw += 2;
                pc += 2;
                if pc + 1 < code.len() {
                    let count = u16::from_le_bytes([code[pc], code[pc+1]]) as usize;
                    pc += 2 + count * 2;
                }
                continue;
            }
            Op::Atomic => { tw += 2; }
            Op::MakeRange => { tw += 1; }

            // Neutral
            Op::Return | Op::ReturnVoid | Op::Pop | Op::Dup | Op::CheckCancel => {}
            Op::DefineFunction => { pc += 4; continue; }
            Op::PushScope | Op::PopScope => {}

            // Float: JIT can handle but not as big a win
            Op::LoadFloat => { jit += 1; pc += 8; continue; }
            Op::GetLocalInt => { jit += 1; pc += 2; continue; }
        }
    }

    (jit, tw)
}
