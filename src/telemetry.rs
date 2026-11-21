use crate::services::command;
use std::{fs, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Memory,
    Network,
}

impl Metric {
    pub fn title(self) -> &'static str {
        match self {
            Self::Cpu => "Top CPU processes",
            Self::Memory => "Memory usage",
            Self::Network => "Network interfaces",
        }
    }
}

pub async fn metric_details(metric: Metric) -> Vec<String> {
    match metric {
        Metric::Cpu => command("ps", &["-eo", "comm,%cpu", "--sort=-%cpu"])
            .await
            .map(|output| output.lines().take(7).map(str::to_owned).collect())
            .unwrap_or_else(|err| vec![err]),
        Metric::Memory => {
            let data = fs::read_to_string("/proc/meminfo").unwrap_or_default();
            let value = |key: &str| {
                data.lines()
                    .find_map(|line| line.strip_prefix(key))
                    .and_then(|line| line.split_whitespace().next())
                    .and_then(|number| number.parse::<u64>().ok())
            };
            match (value("MemTotal:"), value("MemAvailable:")) {
                (Some(total), Some(available)) => vec![
                    format!(
                        "Used: {:.1} GiB",
                        total.saturating_sub(available) as f64 / 1_048_576.0
                    ),
                    format!("Total: {:.1} GiB", total as f64 / 1_048_576.0),
                    format!("Available: {:.1} GiB", available as f64 / 1_048_576.0),
                ],
                _ => vec!["Memory information unavailable".into()],
            }
        }
        Metric::Network => command("ip", &["-brief", "address", "show"])
            .await
            .map(|output| {
                output
                    .lines()
                    .filter(|line| !line.starts_with("lo "))
                    .take(6)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_else(|err| vec![err]),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Cpu {
    total: u64,
    idle: u64,
}

impl Cpu {
    pub fn read() -> Option<Self> {
        Self::parse(fs::read_to_string("/proc/stat").ok()?.lines().next()?)
    }

    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split_whitespace();
        if fields.next()? != "cpu" {
            return None;
        }
        let ticks: Vec<_> = fields
            .take(8)
            .map(str::parse::<u64>)
            .collect::<Result<_, _>>()
            .ok()?;
        let idle = ticks
            .get(3)?
            .checked_add(ticks.get(4).copied().unwrap_or(0))?;
        Some(Self {
            total: ticks.into_iter().sum(),
            idle,
        })
    }

    pub fn usage(self, previous: Self) -> Option<u16> {
        let elapsed = self.total.checked_sub(previous.total)?;
        let idle = self.idle.saturating_sub(previous.idle);
        (elapsed != 0)
            .then(|| (100.0 * elapsed.saturating_sub(idle) as f64 / elapsed as f64).round() as u16)
    }
}

#[derive(Debug, Clone, Default)]
pub enum Network {
    #[default]
    Offline,
    Wired,
    Wifi {
        ssid: String,
        signal: Option<u16>,
    },
    Other(String),
}

pub async fn network() -> Network {
    let ipv4 = fs::read_to_string("/proc/net/route").unwrap_or_default();
    let ipv6 = fs::read_to_string("/proc/net/ipv6_route").unwrap_or_default();
    let interface = ipv4
        .lines()
        .skip(1)
        .filter_map(ipv4_default)
        .min_by_key(|(metric, _)| *metric)
        .or_else(|| {
            ipv6.lines()
                .filter_map(ipv6_default)
                .min_by_key(|(metric, _)| *metric)
        });
    let Some((_, name)) = interface else {
        return Network::Offline;
    };
    let path = Path::new("/sys/class/net").join(name);
    if path.join("wireless").exists() || path.join("phy80211").exists() {
        if let Ok(list) = command(
            "nmcli",
            &[
                "--terse",
                "--fields",
                "IN-USE,SSID,SIGNAL",
                "device",
                "wifi",
                "list",
                "ifname",
                name,
                "--rescan",
                "no",
            ],
        )
        .await
            && let Some((ssid, signal)) = list.lines().find_map(wifi_row)
        {
            return Network::Wifi { ssid, signal };
        }
        let ssid = command("iwgetid", &[name, "--raw"])
            .await
            .unwrap_or_default();
        let signal = fs::read_to_string("/proc/net/wireless")
            .ok()
            .and_then(|data| {
                let (_, stats) = data
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(interface, _)| interface.trim() == name)?;
                stats
                    .split_whitespace()
                    .nth(1)?
                    .parse::<f32>()
                    .ok()
                    .map(|quality| (quality * 100.0 / 70.0).round().clamp(0.0, 100.0) as u16)
            });
        return Network::Wifi { ssid, signal };
    }
    if path.join("device").exists() || name.starts_with("eth") || name.starts_with("en") {
        Network::Wired
    } else {
        Network::Other(name.to_owned())
    }
}

fn usable_flags(text: &str) -> bool {
    u32::from_str_radix(text, 16).is_ok_and(|value| value & 0x201 == 1)
}

fn ipv4_default(line: &str) -> Option<(u32, &str)> {
    let fields: Vec<_> = line.split_whitespace().collect();
    if let [name, "00000000", _, flags, _, _, metric, "00000000", ..] = fields.as_slice()
        && *name != "lo"
        && usable_flags(flags)
    {
        return Some((metric.parse().ok()?, name));
    }
    None
}

fn ipv6_default(line: &str) -> Option<(u32, &str)> {
    let fields: Vec<_> = line.split_whitespace().collect();
    if let [
        "00000000000000000000000000000000",
        "00",
        _,
        "00",
        _,
        metric,
        _,
        _,
        flags,
        name,
    ] = fields.as_slice()
        && *name != "lo"
        && usable_flags(flags)
    {
        return Some((u32::from_str_radix(metric, 16).ok()?, name));
    }
    None
}

fn wifi_row(line: &str) -> Option<(String, Option<u16>)> {
    let (encoded, quality) = line.strip_prefix("*:")?.rsplit_once(':')?;
    let mut ssid = String::new();
    let mut chars = encoded.chars();
    while let Some(c) = chars.next() {
        ssid.push(if c == '\\' { chars.next()? } else { c });
    }
    Some((
        ssid,
        quality
            .parse::<u16>()
            .ok()
            .filter(|percent| *percent <= 100),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Balanced,
    Performance,
    Saver,
}

impl Power {
    const ORDER: [Self; 3] = [Self::Balanced, Self::Performance, Self::Saver];

    fn key(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Performance => "performance",
            Self::Saver => "power-saver",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Performance => "Performance",
            Self::Saver => "Power saver",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PowerProfiles {
    pub active: Power,
    supported: Vec<Power>,
}

impl PowerProfiles {
    pub async fn read() -> Result<Self, String> {
        let out = command("powerprofilesctl", &["list"]).await?;
        Self::parse(&out).ok_or_else(|| "Could not read the active power profile".into())
    }

    fn parse(out: &str) -> Option<Self> {
        let mut active = None;
        let mut supported = Vec::new();
        for line in out.lines().map(str::trim) {
            let (current, label) = line
                .strip_prefix('*')
                .map_or((false, line), |text| (true, text.trim()));
            if let Some(profile) = Power::ORDER
                .into_iter()
                .find(|profile| label.strip_suffix(':') == Some(profile.key()))
            {
                supported.push(profile);
                if current {
                    active = Some(profile);
                }
            }
        }
        Some(Self {
            active: active?,
            supported,
        })
    }

    fn next(&self) -> Option<Power> {
        Power::ORDER
            .into_iter()
            .cycle()
            .skip_while(|&profile| profile != self.active)
            .skip(1)
            .take(Power::ORDER.len() - 1)
            .find(|profile| self.supported.contains(profile))
    }

    pub async fn cycle() -> Result<Self, String> {
        let current = Self::read().await?;
        if let Some(next) = current.next() {
            command("powerprofilesctl", &["set", next.key()]).await?;
        }
        Self::read().await
    }

    pub fn can_cycle(&self) -> bool {
        self.next().is_some()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn balanced_profile() -> Option<PowerProfiles> {
        PowerProfiles::parse("* balanced:\n  power-saver:")
    }

    #[test]
    fn cpu_uses_counter_delta_without_counting_guest_twice() {
        let old = Cpu::parse("cpu 100 20 10 200 10 5 5 0 20 5").unwrap();
        let new = Cpu::parse("cpu 130 20 20 260 10 5 5 0 40 5").unwrap();
        assert_eq!(new.usage(old), Some(40));
        assert_eq!(old.usage(new), None);
    }

    #[test]
    fn routes_and_wifi_names_accept_normal_linux_output() {
        assert_eq!(
            ipv4_default("enp2s0 00000000 0101A8C0 0003 0 0 100 00000000 0 0 0"),
            Some((100, "enp2s0"))
        );
        assert!(ipv6_default("00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200 lo").is_none());
        assert_eq!(
            wifi_row(r"*:lab\:west\\2:77"),
            Some(("lab:west\\2".into(), Some(77)))
        );
    }

    #[test]
    fn cycles_only_through_profiles_reported_by_the_daemon() {
        let state =
            PowerProfiles::parse("  power-saver:\n* balanced:\n    Driver: placeholder").unwrap();
        assert_eq!(state.next(), Some(Power::Saver));
        assert!(!PowerProfiles::parse("* balanced:").unwrap().can_cycle());
    }
}
