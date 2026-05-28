//! System linker detection and invocation for AOT compilation.

use std::process::Command;

/// Link an object file into a standalone executable.
///
/// Uses the system linker (cc/gcc/clang on Linux, cc on macOS, link.exe on Windows)
/// and links against the dgsh runtime library (libdelegate_shell).
pub fn link(object_file: &str, output: &str) -> Result<(), String> {
    let runtime_lib_path = find_runtime_lib()?;
    let linker = find_linker()?;

    let mut cmd = Command::new(&linker);

    if cfg!(target_os = "windows") {
        cmd.arg(object_file)
            .arg(&runtime_lib_path)
            .arg(format!("/OUT:{output}"));
    } else {
        cmd.arg(object_file)
            .arg(&runtime_lib_path)
            .arg("-o")
            .arg(output);

        // System libraries
        if cfg!(target_os = "linux") {
            cmd.args(["-ldl", "-lpthread", "-lm", "-lgcc_s"]);
        } else if cfg!(target_os = "macos") {
            cmd.args(["-lpthread", "-lm", "-framework", "Security"]);
        }
    }

    let result = cmd.output()
        .map_err(|e| format!("Failed to run linker '{}': {e}", linker))?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(format!("Linker failed:\n{stderr}"));
    }

    Ok(())
}

/// Find the runtime library (libdelegate_shell.a).
///
/// Searches relative to the current executable, then in common build paths.
fn find_runtime_lib() -> Result<String, String> {
    // 1. Try relative to the current executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // Same directory as dgsh binary (typical for cargo build)
            let candidate = exe_dir.join("libdelegate_shell.a");
            if candidate.exists() {
                return Ok(candidate.to_string_lossy().to_string());
            }
            // One level up in lib/
            let candidate = exe_dir.join("../lib/libdelegate_shell.a");
            if candidate.exists() {
                return Ok(candidate.canonicalize().unwrap().to_string_lossy().to_string());
            }
        }
    }

    // 2. Platform-specific standard install paths
    let lib_name = if cfg!(target_os = "windows") {
        "delegate_shell.lib"
    } else {
        "libdelegate_shell.a"
    };

    let mut standard_paths: Vec<std::path::PathBuf> = Vec::new();

    if cfg!(target_os = "windows") {
        // %LOCALAPPDATA%\dgsh\
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            standard_paths.push(std::path::PathBuf::from(local).join("dgsh").join(lib_name));
        }
        // %ProgramFiles%\dgsh\
        if let Ok(pf) = std::env::var("ProgramFiles") {
            standard_paths.push(std::path::PathBuf::from(pf).join("dgsh").join(lib_name));
        }
    } else {
        // Linux + macOS
        standard_paths.push(std::path::PathBuf::from("/usr/local/lib/dgsh").join(lib_name));
        standard_paths.push(std::path::PathBuf::from("/usr/lib/dgsh").join(lib_name));
    }

    // ~/.dgsh/lib/ (all platforms)
    if let Ok(home) = std::env::var(if cfg!(target_os = "windows") { "USERPROFILE" } else { "HOME" }) {
        standard_paths.push(std::path::PathBuf::from(home).join(".dgsh").join("lib").join(lib_name));
    }

    for p in &standard_paths {
        if p.exists() {
            return Ok(p.to_string_lossy().to_string());
        }
    }

    // 3. Try target/release (development)
    let dev_candidates = [
        format!("target/release/{lib_name}"),
        format!("target/debug/{lib_name}"),
    ];
    for c in &dev_candidates {
        if std::path::Path::new(c).exists() {
            return Ok(c.to_string());
        }
    }

    Err(format!(
        "Cannot find {lib_name} runtime library.\n\
         Install it or build from source with: cargo build --release"
    ))
}

/// Detect an available system linker.
fn find_linker() -> Result<String, String> {
    if cfg!(target_os = "windows") {
        for linker in ["link.exe", "lld-link"] {
            if command_exists(linker) {
                return Ok(linker.to_string());
            }
        }
        Err("No linker found. Install Visual Studio Build Tools or LLVM.".to_string())
    } else if cfg!(target_os = "macos") {
        if command_exists("cc") {
            Ok("cc".to_string())
        } else {
            Err("No linker found. Install Xcode Command Line Tools: xcode-select --install".to_string())
        }
    } else {
        // Linux and other Unix
        for linker in ["cc", "gcc", "clang"] {
            if command_exists(linker) {
                return Ok(linker.to_string());
            }
        }
        Err("No linker found. Install gcc or clang:\n  Ubuntu/Debian: apt install build-essential\n  Fedora: dnf install gcc".to_string())
    }
}

fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
