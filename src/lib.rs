// Copyright (c) 2026 Jake Swensen
// SPDX-License-Identifier: MPL-2.0
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Reset nRF devices into DFU mode over USB.
//!
//! This crate sends the Nordic USB DFU trigger, the `DFU_DETACH`-style
//! control transfer understood by applications built with the nRF5 SDK's
//! `nrf_dfu_trigger_usb` library, so a running application reboots into its
//! bootloader, ready for a firmware update by another tool such as `nrfutil`
//! or `nrfdfu`. The firmware transfer itself is out of scope.
//!
//! # Usage
//!
//! ```no_run
//! use nrfdfu_reset::{Filter, ResetOptions};
//!
//! // Find devices (defaults to Nordic's VID 0x1915 when the filter is empty).
//! for device in nrfdfu_reset::list(Filter::default())? {
//!     println!("{device} trigger: {}", device.has_trigger_interface());
//! }
//!
//! // Reset the single matching device into DFU mode and wait for it to return.
//! let outcome = nrfdfu_reset::reset(Filter::default(), &ResetOptions::default())?;
//! println!("back as {:04x}:{:04x}", outcome.dfu_vid, outcome.dfu_pid);
//! # Ok::<(), nrfdfu_reset::Error>(())
//! ```
//!
//! # Semantics worth knowing
//!
//! - [`reset`] refuses to act unless exactly one device matching the
//!   filter exposes the DFU trigger interface ([`Error::NoDevice`] /
//!   [`Error::MultipleDevices`]).
//! - The device reboots in the middle of the detach request, so a stalled or
//!   disconnected transfer is the success signal, not an error.
//! - After the detach, the same physical device is recognized by its USB port
//!   path (bus plus hub port chain). The bus address, PID, and serial string
//!   all may change when the bootloader enumerates.
//! - The application firmware version is read from the trigger interface and
//!   logged (via the [`log`] facade) before the reset, best-effort.
//!
//! # Linux permissions
//!
//! Opening the vendor interface usually requires a udev rule granting access.
//! The following example covers the default Nordic VID.
//!
//! ```text
//! SUBSYSTEM=="usb", ATTRS{idVendor}=="1915", MODE="0664", GROUP="plugdev"
//! ```

mod discovery;
mod error;
mod trigger;

pub use discovery::{DeviceMatch, Filter, NORDIC_VID, list};
pub use error::Error;
pub use trigger::{ResetOptions, ResetOutcome, reset};
