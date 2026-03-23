use std::path::PathBuf;

use delegate_shell::Runtime;

/// Find and load ~/.config/dgsh/init.dgsh on REPL startup.
pub fn load_rc(engine: &mut Runtime) {
    if let Some(path) = find_rc() {
        if let Ok(source) = std::fs::read_to_string(&path) {
            if let Err(e) = engine.run_source(&source) {
                eprintln!("Warning: error in {}: {}", path.display(), e);
            }
        }
    }
}

fn find_rc() -> Option<PathBuf> {
    if let Some(home) = home_dir() {
        let path = home.join(".config").join("dgsh").join("init.dgsh");
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
