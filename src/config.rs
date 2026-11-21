use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bar: Bar,
    pub terminal: Option<String>,
    pub weather: Weather,
    /// Problems found while loading, shown to the user once at startup.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

pub const MODULES: [&str; 10] = [
    "launcher",
    "workspaces",
    "cpu",
    "memory",
    "network",
    "power",
    "volume",
    "battery",
    "clock",
    "actions",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    pub height: u32,
    pub margin: i32,
    pub modules: Vec<String>,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            height: 40,
            margin: 10,
            modules: MODULES.into_iter().map(str::to_owned).collect(),
        }
    }
}

impl Bar {
    pub fn shows(&self, name: &str) -> bool {
        self.modules.iter().any(|module| module == name)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weather {
    pub location: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            // The short name keeps the reason readable in the OSD.
            Ok(text) => Self::parse(&text, "config.toml"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => Self::with_warning(format!("could not read {}: {err}", path.display())),
        }
    }

    fn with_warning(warning: String) -> Self {
        eprintln!("frost: {warning}");
        Self {
            warnings: vec![warning],
            ..Self::default()
        }
    }

    fn parse(text: &str, name: &str) -> Self {
        let mut config: Self = match toml::from_str(text) {
            Ok(config) => config,
            Err(err) => {
                // TOML errors span several lines; the first one names the problem.
                let reason = err.message().lines().next().unwrap_or_default().to_owned();
                let line = err.span().map(|span| {
                    text.as_bytes()[..span.start.min(text.len())]
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count()
                        + 1
                });
                return Self::with_warning(match line {
                    Some(line) => format!("{name}:{line}: {reason}; using defaults"),
                    None => format!("{name}: {reason}; using defaults"),
                });
            }
        };
        let unknown: Vec<_> = config
            .bar
            .modules
            .iter()
            .filter(|module| !MODULES.contains(&module.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            let warning = format!("{name}: unknown bar modules: {}", unknown.join(", "));
            eprintln!("frost: {warning}");
            config.warnings.push(warning);
        }
        config.bar.height = config.bar.height.clamp(32, 80);
        config.bar.margin = config.bar.margin.clamp(0, 48);
        config.terminal = config.terminal.filter(|value| !value.trim().is_empty());
        config.weather.location = config
            .weather
            .location
            .filter(|value| !value.trim().is_empty());
        config
    }

    fn path() -> Option<PathBuf> {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|dir| dir.join("frost/config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses_and_controls_modules() {
        let config: Config = toml::from_str(include_str!("../config.example.toml")).unwrap();
        assert_eq!(config.bar.height, 40);
        assert!(config.bar.shows("workspaces"));
        assert_eq!(config.terminal.as_deref(), Some("foot"));
        assert_eq!(config.weather.location.as_deref(), Some("Lisbon"));
        assert!(
            Config::parse(include_str!("../config.example.toml"), "x")
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn mistakes_produce_visible_warnings() {
        let typo = Config::parse("[bar]\nheigth = 40\n", "config.toml");
        assert_eq!(typo.bar.height, 40);
        assert!(typo.warnings[0].starts_with("config.toml:2: unknown field `heigth`"));

        let module = Config::parse("[bar]\nmodules = [\"clock\", \"clok\"]\n", "c");
        assert!(module.bar.shows("clock"));
        assert_eq!(module.warnings, ["c: unknown bar modules: clok"]);
    }
}
