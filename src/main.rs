use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use nrfdfu_reset::{DeviceMatch, Filter, ResetOptions};

#[derive(Debug, Parser)]
#[command(
    name = "nrfdfu-reset",
    version,
    about = "Reset nRF devices into DFU mode via the Nordic USB DFU trigger interface"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List USB devices matching the filter
    List {
        #[command(flatten)]
        filter: FilterArgs,

        /// Print the devices as a JSON array instead of a table
        #[arg(long)]
        json: bool,
    },

    /// Reset the single matching device into DFU mode and wait for it to return
    Reset {
        #[command(flatten)]
        filter: FilterArgs,

        /// Seconds to wait for the device to re-enumerate after the trigger
        #[arg(long, default_value_t = 5.0)]
        timeout: f64,
    },
}

#[derive(Args, Debug)]
struct FilterArgs {
    /// Vendor ID in hex, for example 0x1915 or 1915.
    ///
    /// Defaults to Nordic's 0x1915.
    #[arg(long, value_parser = parse_hex_u16)]
    vid: Option<u16>,

    /// Product ID in hex.
    ///
    /// Requires --vid.
    #[arg(long, value_parser = parse_hex_u16, requires = "vid")]
    pid: Option<u16>,

    /// Physical port path to match exactly.
    ///
    /// Uses the form printed by list, for example 01-1.4.
    #[arg(long)]
    port: Option<String>,

    /// USB serial number to match exactly.
    #[arg(long)]
    serial: Option<String>,
}

impl From<FilterArgs> for Filter {
    fn from(args: FilterArgs) -> Self {
        Filter {
            vid: args.vid,
            pid: args.pid,
            port: args.port,
            serial: args.serial,
        }
    }
}

/// Render the devices as a table with a header row, one line per entry.
fn table_lines(devices: &[DeviceMatch]) -> Vec<String> {
    let width = |header: &str, values: &mut dyn Iterator<Item = usize>| {
        values
            .chain(std::iter::once(header.len()))
            .max()
            .unwrap_or(0)
    };
    let port_width = width("PORT", &mut devices.iter().map(|d| d.port_path().len()));
    let serial_width = width(
        "SERIAL",
        &mut devices
            .iter()
            .map(|d| d.serial.as_deref().unwrap_or("-").len()),
    );
    let mut lines = vec![format!(
        "{:<4}  {:<4}  {:<port_width$}  {:<serial_width$}  {:<7}  {}",
        "VID", "PID", "PORT", "SERIAL", "TRIGGER", "PRODUCT"
    )];
    for device in devices {
        lines.push(format!(
            "{:04x}  {:04x}  {:<port_width$}  {:<serial_width$}  {:<7}  {}",
            device.vid,
            device.pid,
            device.port_path(),
            device.serial.as_deref().unwrap_or("-"),
            if device.has_trigger_interface() {
                "yes"
            } else {
                "no"
            },
            device.product.as_deref().unwrap_or("-"),
        ));
    }
    lines
}

/// Render the devices as a JSON array.
fn devices_json(devices: &[DeviceMatch]) -> String {
    let entries: Vec<serde_json::Value> = devices
        .iter()
        .map(|device| {
            serde_json::json!({
                "vid": format!("{:04x}", device.vid),
                "pid": format!("{:04x}", device.pid),
                "port_path": device.port_path(),
                "has_trigger_interface": device.has_trigger_interface(),
                "serial": device.serial,
                "product": device.product,
            })
        })
        .collect();
    serde_json::Value::Array(entries).to_string()
}

fn parse_hex_u16(value: &str) -> Result<u16, String> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u16::from_str_radix(digits, 16).map_err(|e| format!("invalid hex value {value:?}: {e}"))
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), nrfdfu_reset::Error> {
    match cli.command {
        Command::List { filter, json } => {
            let devices = nrfdfu_reset::list(filter.into())?;
            if json {
                println!("{}", devices_json(&devices));
            } else if devices.is_empty() {
                println!("no matching devices");
            } else {
                for line in table_lines(&devices) {
                    println!("{line}");
                }
            }
            Ok(())
        }
        Command::Reset { filter, timeout } => {
            let options = ResetOptions {
                timeout: Duration::from_secs_f64(timeout),
                ..ResetOptions::default()
            };
            let outcome = nrfdfu_reset::reset(filter.into(), &options)?;
            println!(
                "device re-enumerated as {:04x}:{:04x} (DFU mode)",
                outcome.dfu_vid, outcome.dfu_pid
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_without_vid_is_a_usage_error() {
        let result = Cli::try_parse_from(["nrfdfu-reset", "list", "--pid", "521f"]);
        let error = result.expect_err("--pid without --vid must be rejected");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );

        let result = Cli::try_parse_from(["nrfdfu-reset", "reset", "--pid", "521f"]);
        assert!(result.is_err());
    }

    fn device(serial: Option<&str>, product: Option<&str>) -> DeviceMatch {
        DeviceMatch {
            vid: 0x1915,
            pid: 0x154B,
            bus_id: "01".into(),
            port_chain: vec![1, 4],
            device_address: 5,
            trigger_interface: Some(2),
            serial: serial.map(str::to_string),
            product: product.map(str::to_string),
        }
    }

    #[test]
    fn hex_ids_parse_with_and_without_prefix() {
        let cli = Cli::try_parse_from(["nrfdfu-reset", "list", "--vid", "0x1915", "--pid", "521F"])
            .unwrap();
        let Command::List { filter, json } = cli.command else {
            panic!("expected list subcommand");
        };
        assert_eq!(filter.vid, Some(0x1915));
        assert_eq!(filter.pid, Some(0x521F));
        assert!(!json);
    }

    #[test]
    fn list_accepts_the_json_flag() {
        let cli = Cli::try_parse_from(["nrfdfu-reset", "list", "--json"]).unwrap();
        let Command::List { json, .. } = cli.command else {
            panic!("expected list subcommand");
        };
        assert!(json);
    }

    #[test]
    fn port_and_serial_selectors_parse_standalone() {
        let cli = Cli::try_parse_from(["nrfdfu-reset", "reset", "--port", "01-1.4"]).unwrap();
        let Command::Reset { filter, .. } = cli.command else {
            panic!("expected reset subcommand");
        };
        assert_eq!(filter.port.as_deref(), Some("01-1.4"));
        assert_eq!(filter.vid, None);

        let cli = Cli::try_parse_from(["nrfdfu-reset", "list", "--serial", "ABC123"]).unwrap();
        let Command::List { filter, .. } = cli.command else {
            panic!("expected list subcommand");
        };
        assert_eq!(filter.serial.as_deref(), Some("ABC123"));
    }

    #[test]
    fn table_starts_with_a_header_row_and_aligns_columns() {
        let lines = table_lines(&[
            device(Some("9E4D0483F708B8EB"), Some("nRF 802154 Sniffer")),
            device(None, None),
        ]);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("VID   PID   PORT"), "{}", lines[0]);
        assert!(lines[0].contains("SERIAL"), "{}", lines[0]);
        assert!(lines[0].contains("TRIGGER  PRODUCT"), "{}", lines[0]);
        assert!(lines[1].starts_with("1915  154b  01-1.4"), "{}", lines[1]);
        assert!(lines[1].contains("9E4D0483F708B8EB"), "{}", lines[1]);
        assert!(lines[1].ends_with("nRF 802154 Sniffer"), "{}", lines[1]);
        assert!(lines[2].contains("  -  "), "{}", lines[2]);
        assert!(lines[2].ends_with(" -"), "{}", lines[2]);
    }

    #[test]
    fn json_output_holds_every_field_and_parses() {
        let text = devices_json(&[device(Some("ABC123"), Some("nRF \"quoted\" name"))]);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entry = &value.as_array().unwrap()[0];
        assert_eq!(entry["vid"], "1915");
        assert_eq!(entry["pid"], "154b");
        assert_eq!(entry["port_path"], "01-1.4");
        assert_eq!(entry["has_trigger_interface"], true);
        assert_eq!(entry["serial"], "ABC123");
        assert_eq!(entry["product"], "nRF \"quoted\" name");

        let text = devices_json(&[device(None, None)]);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value.as_array().unwrap()[0]["serial"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn json_output_is_an_empty_array_with_no_devices() {
        assert_eq!(devices_json(&[]), "[]");
    }

    #[test]
    fn reset_timeout_defaults_to_five_seconds() {
        let cli = Cli::try_parse_from(["nrfdfu-reset", "reset"]).unwrap();
        let Command::Reset { timeout, .. } = cli.command else {
            panic!("expected reset subcommand");
        };
        assert_eq!(timeout, 5.0);
    }

    #[test]
    fn help_is_available_for_both_subcommands() {
        for sub in ["list", "reset"] {
            let error = Cli::try_parse_from(["nrfdfu-reset", sub, "--help"]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
        }
    }
}
