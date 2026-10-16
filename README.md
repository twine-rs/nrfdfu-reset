# nrfdfu-reset

Reset nRF devices into DFU mode over USB.

This crate — a small library plus a CLI — sends the Nordic USB DFU trigger
(the `DFU_DETACH`-style control transfer understood by applications built with
the nRF5 SDK's `nrf_dfu_trigger_usb` library) so a running application reboots
into its bootloader. It then verifies the device re-enumerated at the same
physical USB port and reports what it came back as.

**Scope:** triggering DFU mode only. Other tools, such as
[`nrfutil`](https://www.nordicsemi.com/Products/Development-tools/nrf-util)
or [`nrfdfu`](https://github.com/knurling-rs/nrfdfu-rs), perform the actual
firmware transfer:

```sh
nrfdfu-reset reset && nrfdfu firmware.elf
```

Built on [`nusb`](https://crates.io/crates/nusb) — pure Rust, no libusb.

## Install

```sh
cargo install --path .
```

## Usage

List candidate devices (defaults to Nordic's vendor ID `0x1915`):

```sh
nrfdfu-reset list
nrfdfu-reset list --vid 1915
nrfdfu-reset list --vid 1915 --pid 521f
```

Output shows VID:PID, the physical port path, whether the device exposes the
DFU trigger interface, and the product string:

```text
1915:520f  port 01-1.4        trigger: yes  nRF52 Connectivity
```

Reset the matching device into DFU mode:

```sh
nrfdfu-reset reset
nrfdfu-reset reset --vid 1915 --pid 520f --timeout 10
```

`reset` only proceeds when **exactly one** device matching the filter exposes
the trigger interface. Zero or multiple matches exit non-zero with a message
listing the candidates so you can narrow the filter. `--pid` requires `--vid`.
VID/PID values are hexadecimal, with or without a `0x` prefix.

Before the reset, the tool reads the application's firmware version from the
trigger interface and logs it (best effort). After the reset, the tool waits
(default 5 s, see `--timeout`) for a device to re-appear at the same USB port
and prints the VID:PID it re-enumerated with. A non-zero exit with "DFU trigger
sent, but no device re-appeared" means the tool delivered the detach but did
not observe re-enumeration. The device may still be in DFU mode.

### Logging

Diagnostics go through [`env_logger`]. Control verbosity with `RUST_LOG`:

```sh
RUST_LOG=debug nrfdfu-reset reset
```

[`env_logger`]: https://crates.io/crates/env_logger

## Library

```rust,no_run
use nrfdfu_reset::{Filter, ResetOptions};

let outcome = nrfdfu_reset::reset(Filter::default(), &ResetOptions::default())?;
println!("back as {:04x}:{:04x}", outcome.dfu_vid, outcome.dfu_pid);
# Ok::<(), nrfdfu_reset::Error>(())
```

The library only logs via the [`log`](https://crates.io/crates/log) facade and
stays silent unless the application installs a logger.

## Linux permissions

Opening the vendor interface usually requires a udev rule, e.g. for the
default Nordic VID (`/etc/udev/rules.d/71-nrf-dfu-trigger.rules`):

```text
SUBSYSTEM=="usb", ATTRS{idVendor}=="1915", MODE="0664", GROUP="plugdev"
```

Reload with `sudo udevadm control --reload-rules && sudo udevadm trigger`.

## License

[MPL-2.0](LICENSE)
