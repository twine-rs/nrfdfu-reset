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
/// With no fields set, discovery defaults to [`NORDIC_VID`]. A `pid` without a
/// `vid` is rejected with [`Error::InvalidFilter`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Filter {
    /// Vendor ID to match.
    ///
    /// `None` defaults to [`NORDIC_VID`] unless `pid` is set, which is
    /// invalid.
    pub vid: Option<u16>,

    /// Product ID to match.
    ///
    /// Requires `vid`.
    pub pid: Option<u16>,
}

impl Filter {
    /// Resolve the filter to the concrete `(vid, Option<pid>)` to match against.
    pub(crate) fn resolve(self) -> Result<(u16, Option<u16>), Error> {
        match (self.vid, self.pid) {
            (None, Some(_)) => Err(Error::InvalidFilter),
            (None, None) => Ok((NORDIC_VID, None)),
            (Some(vid), pid) => Ok((vid, pid)),
        }
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
    let (vid, pid) = filter.resolve()?;
    // nusb 0.2.8 provides `DeviceInfo::bus_id() -> &str` and
    // `DeviceInfo::port_chain() -> &[u8]` on Linux, macOS, and Windows.
    // Together they identify the physical port across a re-enumeration.
    // `device_address()` changes on re-enumeration and is used to detect
    // that the device at the port is a new instance.
    let devices = nusb::list_devices().wait()?;
    Ok(devices
        .filter(|info| info.vendor_id() == vid && pid.is_none_or(|p| info.product_id() == p))
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
                product: info.product_string().map(str::to_string),
            };
            (info, device)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_defaults_to_nordic_vid() {
        assert_eq!(Filter::default().resolve().unwrap(), (NORDIC_VID, None));
    }

    #[test]
    fn vid_only_filter_matches_any_pid() {
        let filter = Filter {
            vid: Some(0x2FE3),
            pid: None,
        };
        assert_eq!(filter.resolve().unwrap(), (0x2FE3, None));
    }

    #[test]
    fn vid_and_pid_filter_resolves_both() {
        let filter = Filter {
            vid: Some(0x2FE3),
            pid: Some(0x0100),
        };
        assert_eq!(filter.resolve().unwrap(), (0x2FE3, Some(0x0100)));
    }

    #[test]
    fn pid_without_vid_is_invalid() {
        let filter = Filter {
            vid: None,
            pid: Some(0x0100),
        };
        assert!(matches!(filter.resolve(), Err(Error::InvalidFilter)));
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
            product: None,
        };
        assert_eq!(device.port_path(), "20-1.3.2");
    }
}
