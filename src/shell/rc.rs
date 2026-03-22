use std::path::PathBuf;

use delegate_shell::Interpreter;

/// Find and load .dgshrc from the user's home directory.
pub fn load_rc(engine: &mut Interpreter) {
    if let Some(path) = find_rc() {
        if let Ok(source) = std::fs::read_to_string(&path) {
            if let Err(e) = engine.run_source(&source) {
                eprintln!("Warning: error in {}: {}", path.display(), e);
            }
        }
    }
}

fn find_rc() -> Option<PathBuf> {
    // Check $DGSH_RC first
    if let Ok(custom) = std::env::var("DGSH_RC") {
        let path = PathBuf::from(custom);
        if path.exists() {
            return Some(path);
        }
    }

    // Then ~/.dgshrc
    if let Some(home) = home_dir() {
        let path = home.join(".dgshrc");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}
