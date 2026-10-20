// Copyright (c) 2026 Jake Swensen
// SPDX-License-Identifier: MPL-2.0
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use std::fmt;

use nusb::MaybeFuture;

use crate::error::Error;

/// Nordic Semiconductor's USB vendor ID, the default discovery filter.
pub const NORDIC_VID: u16 = 0x1915;

// The Nordic DFU trigger interface is vendor-specific class 0xFF with
// subclass 0x01 and protocol 0x01, matching pc-nrfutil's identification rule.
const TRIGGER_INTERFACE_CLASS: u8 = 0xFF;
const TRIGGER_INTERFACE_SUBCLASS: u8 = 0x01;
const TRIGGER_INTERFACE_PROTOCOL: u8 = 0x01;

/// Device selection filter.
///
/// All supplied fields combine conjunctively. With no fields set, discovery
/// defaults to [`NORDIC_VID`]. A supplied `port` or `serial` stands alone and
/// does not imply the default vendor ID. A `pid` without a `vid` is rejected
/// with [`Error::InvalidFilter`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter {
    /// Vendor ID to match.
    ///
    /// `None` defaults to [`NORDIC_VID`] when every other field is also
    /// `None`.
    pub vid: Option<u16>,

    /// Product ID to match.
    ///
    /// Requires `vid`.
    pub pid: Option<u16>,

    /// Physical port path to match exactly.
    ///
    /// Uses the form reported by [`DeviceMatch::port_path`], for example
    /// `01-1.4`.
    pub port: Option<String>,

    /// USB serial number to match exactly.
    ///
    /// A device that reports no serial number never matches.
    pub serial: Option<String>,
}

/// A [`Filter`] with the default vendor ID applied.
pub(crate) struct ResolvedFilter<'a> {
    vid: Option<u16>,
    pid: Option<u16>,
    port: Option<&'a str>,
    serial: Option<&'a str>,
}

impl Filter {
    /// Apply the resolution rules and reject invalid field combinations.
    pub(crate) fn resolve(&self) -> Result<ResolvedFilter<'_>, Error> {
        if self.pid.is_some() && self.vid.is_none() {
            return Err(Error::InvalidFilter);
        }
        let nothing_supplied = self.vid.is_none()
            && self.pid.is_none()
            && self.port.is_none()
            && self.serial.is_none();
        Ok(ResolvedFilter {
            vid: if nothing_supplied {
                Some(NORDIC_VID)
            } else {
                self.vid
            },
            pid: self.pid,
            port: self.port.as_deref(),
            serial: self.serial.as_deref(),
        })
    }
}

impl ResolvedFilter<'_> {
    /// Whether `device` satisfies every supplied field.
    pub(crate) fn matches(&self, device: &DeviceMatch) -> bool {
        self.vid.is_none_or(|vid| device.vid == vid)
            && self.pid.is_none_or(|pid| device.pid == pid)
            && self.port.is_none_or(|port| device.port_path() == port)
            && self
                .serial
                .is_none_or(|serial| device.serial.as_deref() == Some(serial))
    }
}

/// A USB device that matched the discovery filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceMatch {
    /// USB vendor ID.
    pub vid: u16,

    /// USB product ID.
    pub pid: u16,

    /// Identifier of the bus the device is attached to.
    pub bus_id: String,

    /// Hub port numbers from the root port down to the device.
    pub port_chain: Vec<u8>,

    /// Bus address assigned at enumeration (changes when the device re-enumerates).
    pub device_address: u8,

    /// Interface number of the Nordic DFU trigger interface, if the device exposes one.
    pub trigger_interface: Option<u8>,

    /// USB serial number string, if the device reports one.
    ///
    /// Serial numbers are not guaranteed stable across the application to
    /// bootloader transition, so they select devices before a reset and play
    /// no role in post-reset verification.
    pub serial: Option<String>,

    /// USB product string, if reported.
    pub product: Option<String>,
}

impl DeviceMatch {
    /// Whether the device exposes the Nordic DFU trigger interface.
    pub fn has_trigger_interface(&self) -> bool {
        self.trigger_interface.is_some()
    }

    /// Stable identifier for the physical USB port (`<bus>-<port>.<port>...`).
    ///
    /// Unlike the device address, PID, or serial string, this does not change
    /// when the device reboots into its bootloader on the same port.
    pub fn port_path(&self) -> String {
        let chain: Vec<String> = self.port_chain.iter().map(u8::to_string).collect();
        format!("{}-{}", self.bus_id, chain.join("."))
    }
}

impl fmt::Display for DeviceMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04x}:{:04x} at {}",
            self.vid,
            self.pid,
            self.port_path()
        )?;
        if let Some(product) = &self.product {
            write!(f, " ({product})")?;
        }
        Ok(())
    }
}

/// Predicate for the Nordic DFU trigger interface descriptor values.
pub(crate) fn is_trigger_interface(class: u8, subclass: u8, protocol: u8) -> bool {
    class == TRIGGER_INTERFACE_CLASS
        && subclass == TRIGGER_INTERFACE_SUBCLASS
        && protocol == TRIGGER_INTERFACE_PROTOCOL
}

/// List USB devices matching `filter`.
///
/// Returns every filter match, including devices that do not expose the DFU
/// trigger interface (see [`DeviceMatch::has_trigger_interface`]).
pub fn list(filter: Filter) -> Result<Vec<DeviceMatch>, Error> {
    Ok(list_with_info(filter)?
        .into_iter()
        .map(|(_, device)| device)
        .collect())
}

/// Like [`list`], but keeps the `nusb::DeviceInfo` needed to open each device.
pub(crate) fn list_with_info(
    filter: Filter,
) -> Result<Vec<(nusb::DeviceInfo, DeviceMatch)>, Error> {
    let resolved = filter.resolve()?;
    // nusb 0.2.8 provides `DeviceInfo::bus_id() -> &str` and
    // `DeviceInfo::port_chain() -> &[u8]` on Linux, macOS, and Windows.
    // Together they identify the physical port across a re-enumeration.
    // `device_address()` changes on re-enumeration and is used to detect
    // that the device at the port is a new instance.
    let devices = nusb::list_devices().wait()?;
    Ok(devices
        .map(|info| {
            let device = DeviceMatch {
                vid: info.vendor_id(),
                pid: info.product_id(),
                bus_id: info.bus_id().to_string(),
                port_chain: info.port_chain().to_vec(),
                device_address: info.device_address(),
                trigger_interface: info
                    .interfaces()
                    .find(|i| is_trigger_interface(i.class(), i.subclass(), i.protocol()))
                    .map(|i| i.interface_number()),
                serial: info.serial_number().map(str::to_string),
                product: info.product_string().map(str::to_string),
            };
            (info, device)
        })
        .filter(|(_, device)| resolved.matches(device))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(vid: u16, port: u8, serial: Option<&str>) -> DeviceMatch {
        DeviceMatch {
            vid,
            pid: 0x520F,
            bus_id: "20".into(),
            port_chain: vec![1, port],
            device_address: 5,
            trigger_interface: None,
            serial: serial.map(str::to_string),
            product: None,
        }
    }

    fn matches(filter: &Filter, device: &DeviceMatch) -> bool {
        filter.resolve().unwrap().matches(device)
    }

    #[test]
    fn empty_filter_defaults_to_nordic_vid() {
        let filter = Filter::default();
        assert!(matches(&filter, &device(NORDIC_VID, 1, None)));
        assert!(!matches(&filter, &device(0x2FE3, 1, None)));
    }

    #[test]
    fn vid_only_filter_matches_any_pid() {
        let filter = Filter {
            vid: Some(0x2FE3),
            ..Filter::default()
        };
        assert!(matches(&filter, &device(0x2FE3, 1, None)));
        assert!(!matches(&filter, &device(NORDIC_VID, 1, None)));
    }

    #[test]
    fn vid_and_pid_filter_requires_both() {
        let filter = Filter {
            vid: Some(NORDIC_VID),
            pid: Some(0x0100),
            ..Filter::default()
        };
        assert!(!matches(&filter, &device(NORDIC_VID, 1, None)));
    }

    #[test]
    fn pid_without_vid_is_invalid() {
        let filter = Filter {
            pid: Some(0x0100),
            ..Filter::default()
        };
        assert!(matches!(filter.resolve(), Err(Error::InvalidFilter)));
    }

    #[test]
    fn port_selector_alone_ignores_the_vid_default() {
        let filter = Filter {
            port: Some("20-1.3".into()),
            ..Filter::default()
        };
        assert!(matches(&filter, &device(0x2FE3, 3, None)));
        assert!(!matches(&filter, &device(0x2FE3, 4, None)));
    }

    #[test]
    fn serial_selector_alone_ignores_the_vid_default() {
        let filter = Filter {
            serial: Some("AB12".into()),
            ..Filter::default()
        };
        assert!(matches(&filter, &device(0x2FE3, 1, Some("AB12"))));
        assert!(!matches(&filter, &device(0x2FE3, 1, Some("CD34"))));
    }

    #[test]
    fn missing_serial_never_matches_a_serial_selector() {
        let filter = Filter {
            serial: Some("AB12".into()),
            ..Filter::default()
        };
        assert!(!matches(&filter, &device(NORDIC_VID, 1, None)));
    }

    #[test]
    fn combined_fields_match_conjunctively() {
        let filter = Filter {
            vid: Some(NORDIC_VID),
            port: Some("20-1.3".into()),
            ..Filter::default()
        };
        assert!(matches(&filter, &device(NORDIC_VID, 3, None)));
        assert!(!matches(&filter, &device(0x2FE3, 3, None)));
        assert!(!matches(&filter, &device(NORDIC_VID, 4, None)));
    }

    #[test]
    fn trigger_interface_predicate_matches_nordic_descriptor() {
        assert!(is_trigger_interface(0xFF, 0x01, 0x01));
        // CDC ACM control interface
        assert!(!is_trigger_interface(0x02, 0x02, 0x01));
        // Vendor-specific but wrong subclass/protocol (a J-Link, for example)
        assert!(!is_trigger_interface(0xFF, 0xFF, 0xFF));
        assert!(!is_trigger_interface(0xFF, 0x01, 0x00));
        assert!(!is_trigger_interface(0xFF, 0x00, 0x01));
    }

    #[test]
    fn port_path_formats_bus_and_chain() {
        let device = DeviceMatch {
            vid: 0x1915,
            pid: 0x520F,
            bus_id: "20".into(),
            port_chain: vec![1, 3, 2],
            device_address: 5,
            trigger_interface: None,
            serial: None,
            product: None,
        };
        assert_eq!(device.port_path(), "20-1.3.2");
    }
}
