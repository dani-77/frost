use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bar: Bar,
    pub terminal: Option<String>,
    pub weather: Weather,
}

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
            modules: [
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
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
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
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                eprintln!("frost: could not read {}: {err}", path.display());
                return Self::default();
            }
        };
        let mut config: Self = match toml::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("frost: invalid {}: {err}", path.display());
                return Self::default();
            }
        };
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
    }
}
