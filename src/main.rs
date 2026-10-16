use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use nrfdfu_reset::{Filter, ResetOptions};

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
    List(FilterArgs),

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
}

impl From<FilterArgs> for Filter {
    fn from(args: FilterArgs) -> Self {
        Filter {
            vid: args.vid,
            pid: args.pid,
        }
    }
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
        Command::List(filter) => {
            let devices = nrfdfu_reset::list(filter.into())?;
            if devices.is_empty() {
                println!("no matching devices");
            }
            for device in devices {
                println!(
                    "{:04x}:{:04x}  port {:<12}  trigger: {}  {}",
                    device.vid,
                    device.pid,
                    device.port_path(),
                    if device.has_trigger_interface() {
                        "yes"
                    } else {
                        "no "
                    },
                    device.product.as_deref().unwrap_or("")
                );
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

    #[test]
    fn hex_ids_parse_with_and_without_prefix() {
        let cli = Cli::try_parse_from(["nrfdfu-reset", "list", "--vid", "0x1915", "--pid", "521F"])
            .unwrap();
        let Command::List(filter) = cli.command else {
            panic!("expected list subcommand");
        };
        assert_eq!(filter.vid, Some(0x1915));
        assert_eq!(filter.pid, Some(0x521F));
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
