use rustyline::completion::{Completer, Pair};
use rustyline::Context;

pub struct DgshCompleter {
    builtin_names: Vec<String>,
}

impl DgshCompleter {
    pub fn new(builtin_names: Vec<String>) -> Self {
        Self { builtin_names }
    }

    fn complete_path(&self, partial: &str) -> Vec<Pair> {
        let (dir, prefix) = if let Some(pos) = partial.rfind('/') {
            (&partial[..=pos], &partial[pos + 1..])
        } else {
            ("./", partial)
        };

        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };

        let mut results = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(prefix) {
                let full = if dir == "./" {
                    name.clone()
                } else {
                    format!("{}{}", dir, name)
                };
                let display = if entry.path().is_dir() {
                    format!("{}/", name)
                } else {
                    name
                };
                results.push(Pair {
                    display,
                    replacement: full,
                });
            }
        }
        results.sort_by(|a, b| a.display.cmp(&b.display));
        results
    }

    fn complete_builtin(&self, partial: &str) -> Vec<Pair> {
        self.builtin_names
            .iter()
            .filter(|name| name.starts_with(partial))
            .map(|name| Pair {
                display: name.clone(),
                replacement: name.clone(),
            })
            .collect()
    }
}

impl Completer for DgshCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        let start = before.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1);
        let partial = &before[start..];

        if partial.is_empty() {
            return Ok((pos, Vec::new()));
        }

        // If it contains '/' or '.', complete paths
        if partial.contains('/') || partial.contains('.') {
            return Ok((start, self.complete_path(partial)));
        }

        // Otherwise complete builtins and keywords
        let mut results = self.complete_builtin(partial);

        // Also add path completions
        results.extend(self.complete_path(partial));

        Ok((start, results))
    }
}
