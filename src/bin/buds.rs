use anyhow::Result;
use buds::{protocol::{Eq, Mode, Spatial}, session::Session};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Control OnePlus Buds Pro 3 from macOS")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the current noise-control mode
    Status,
    /// Set a mode: off, transparency, high, medium, low, adaptive
    Set { mode: String },
    /// Show earbud and case battery levels
    Battery,
    /// Print every frame the buds send (decoded) for N seconds
    /// Read-only scan: send query opcodes 0x01..=0x40 (category 0x01) and print any replies
    Probe,
    /// Send one raw frame: raw <lo-hex> <hi-hex> [payload-hex], print replies for 1.5s
    Raw { lo: String, hi: String, payload: Option<String> },
    /// Show or set the equalizer preset: balanced, bold, serenade, bass, dynaudio
    Eq { preset: Option<String> },
    /// Set spatial audio: off, fixed, head
    Spatial { mode: String },
    Listen { #[arg(default_value_t = 30)] seconds: u64 },
}

fn parse_mode(s: &str) -> Option<Mode> {
    Mode::ALL.into_iter().find(|m| format!("{m:?}").eq_ignore_ascii_case(s))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut s = Session::open()?;
    match cli.cmd {
        Cmd::Status => match s.query_mode()? {
            Some(m) => println!("{}", m.label()),
            None => println!("unknown mode"),
        },
        Cmd::Battery => {
            let b = s.query_battery()?;
            let show = |n: &str, c: Option<buds::protocol::Cell>| match c {
                Some(c) => println!("{n}: {}%{}", c.percent, if c.charging { " (charging)" } else { "" }),
                None => println!("{n}: -"),
            };
            show("Left", b.left);
            show("Right", b.right);
            show("Case", b.case);
        }
        Cmd::Set { mode } => {
            let m = parse_mode(&mode).ok_or_else(|| anyhow::anyhow!("unknown mode '{mode}' (off, transparency, high, medium, low, adaptive)"))?;
            match s.set_mode(m)? {
                Some(now) if now == m => println!("OK: {}", now.label()),
                Some(now) => println!("buds report {} (asked for {})", now.label(), m.label()),
                None => println!("sent, but the buds reported an unknown mode"),
            }
        }
        Cmd::Raw { lo, hi, payload } => {
            let lo = u8::from_str_radix(&lo, 16)?;
            let hi = u8::from_str_radix(&hi, 16)?;
            let pl = hex::decode(payload.unwrap_or_default())?;
            let f = s.b.raw(lo, hi, &pl);
            println!("-> {}", hex::encode(&f));
            s.link.send(&f)?;
            let end = std::time::Instant::now() + std::time::Duration::from_millis(1500);
            while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
                if let Ok(raw) = s.link.rx.recv_timeout(left) {
                    println!("<- {}", hex::encode(&raw));
                }
            }
        }
        Cmd::Eq { preset } => match preset {
            None => match s.query_eq()? {
                Some(e) => println!("Equalizer: {}", e.label()),
                None => println!("Equalizer: unknown preset"),
            },
            Some(p) => {
                let e = match p.to_lowercase().as_str() {
                    "balanced" | "balance" => Eq::Balanced,
                    "bold" => Eq::Bold,
                    "serenade" => Eq::Serenade,
                    "bass" | "bassboost" => Eq::BassBoost,
                    "dynaudio" | "dyn" => Eq::DynAudio,
                    _ => anyhow::bail!("unknown preset '{p}' (balanced, bold, serenade, bass, dynaudio)"),
                };
                match s.set_eq(e)? {
                    Some(now) if now == e => println!("OK: Equalizer {}", now.label()),
                    Some(now) => println!("buds report {} (asked for {})", now.label(), e.label()),
                    None => println!("sent, but the buds reported an unknown preset"),
                }
            }
        },
        Cmd::Spatial { mode } => {
            let m = match mode.to_lowercase().as_str() {
                "off" => Spatial::Off,
                "fixed" => Spatial::Fixed,
                "head" | "tracked" | "headtracked" => Spatial::HeadTracked,
                _ => anyhow::bail!("unknown mode '{mode}' (off, fixed, head)"),
            };
            s.set_spatial(m)?;
            println!("OK: Spatial audio {}", m.label());
        }
        Cmd::Probe => {
            for lo in 0x01u8..=0x40 {
                if lo == 0x0C || lo == 0x06 {
                    continue;
                }
                let f = s.b.raw(lo, 0x01, &[]);
                s.link.send(&f)?;
                println!("-> query {lo:02x}01");
                let end = std::time::Instant::now() + std::time::Duration::from_millis(500);
                while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
                    if let Ok(raw) = s.link.rx.recv_timeout(left) {
                        println!("<- {}", hex::encode(&raw));
                    }
                }
            }
        }
        Cmd::Listen { seconds } => {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
            while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
                if let Ok(raw) = s.link.rx.recv_timeout(left) {
                    println!("{}  {:?}", hex::encode(&raw), buds::protocol::decode(&raw));
                }
            }
        }
    }
    Ok(())
}
