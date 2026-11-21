use std::{collections::HashSet, env, fs, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub search: String,
}

/// Index common user folders when the launcher opens. Bounds keep a large
/// home directory from delaying the application launcher indefinitely.
pub fn index() -> Vec<Entry> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut roots = vec![home.clone()];
    if let Ok(config) = fs::read_to_string(home.join(".config/user-dirs.dirs")) {
        for line in config.lines().filter(|line| line.starts_with("XDG_")) {
            if let Some((_, value)) = line.split_once('=') {
                let value = value
                    .trim()
                    .trim_matches('"')
                    .replace("$HOME", &home.to_string_lossy());
                let path = PathBuf::from(value);
                if path.is_dir() && path.starts_with(&home) {
                    roots.push(path);
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    let mut visited = HashSet::new();
    let mut stack = vec![(home, 0_usize)];
    // XDG folders may be links outside the home, but never follow links.
    stack.extend(roots.into_iter().skip(1).map(|root| (root, 0)));
    let mut files = Vec::new();
    while let Some((dir, depth)) = stack.pop() {
        if files.len() >= 20_000 || !visited.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for item in entries.flatten() {
            if files.len() >= 20_000 {
                break;
            }
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || matches!(name.as_str(), "node_modules" | "target" | "vendor")
            {
                continue;
            }
            let Ok(kind) = item.file_type() else { continue };
            if kind.is_dir() && depth < 5 {
                stack.push((path, depth + 1));
            } else if kind.is_file() {
                let search = path.to_string_lossy().to_lowercase();
                files.push(Entry { path, name, search });
            }
        }
    }
    files.sort_by_key(|file| file.name.to_lowercase());
    files
}

pub async fn open(path: PathBuf) -> Result<String, String> {
    crate::services::command("xdg-open", &[&path.to_string_lossy()]).await
}
