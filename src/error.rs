use std::fmt::Write as _;
use std::time::Duration;

use crate::discovery::DeviceMatch;

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A product ID was supplied without a vendor ID.
    #[error("invalid filter: a product ID requires a vendor ID")]
    InvalidFilter,

    /// No device matching the filter exposes the Nordic DFU trigger interface.
    #[error("no device matching the filter exposes the Nordic DFU trigger interface")]
    NoDevice,

    /// The reset target is ambiguous.
    ///
    /// More than one matching device exposes the DFU trigger interface.
    /// Narrow the filter, for example by adding a PID.
    #[error(
        "multiple matching devices expose the DFU trigger interface. Narrow the filter:{}",
        candidate_list(candidates)
    )]
    MultipleDevices {
        /// The devices that matched the filter and expose the trigger interface.
        candidates: Vec<DeviceMatch>,
    },

    /// A USB operation (enumeration, open, claim) failed.
    #[error("USB error: {0}")]
    Usb(#[from] nusb::Error),

    /// A USB control transfer failed.
    ///
    /// The failure does not indicate the expected reboot-into-bootloader
    /// disconnect.
    #[error("USB transfer failed: {0}")]
    Transfer(#[from] nusb::transfer::TransferError),

    /// The detach was delivered but re-enumeration was not observed.
    ///
    /// No device re-appeared at the same USB port within the timeout. The
    /// device may still have entered DFU mode.
    #[error(
        "DFU trigger sent, but no device re-appeared at the same USB port within {waited:?}. \
         The device may still have entered DFU mode"
    )]
    TriggerSentButNotVerified {
        /// How long re-enumeration was polled for before timing out.
        waited: Duration,
    },
}

fn candidate_list(candidates: &[DeviceMatch]) -> String {
    candidates.iter().fold(String::new(), |mut out, device| {
        let _ = write!(out, "\n  - {device}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_devices_message_lists_candidates() {
        let err = Error::MultipleDevices {
            candidates: vec![
                DeviceMatch {
                    vid: 0x1915,
                    pid: 0x520F,
                    bus_id: "20".into(),
                    port_chain: vec![1, 3],
                    device_address: 7,
                    trigger_interface: Some(2),
                    product: Some("nRF52 USB".into()),
                },
                DeviceMatch {
                    vid: 0x1915,
                    pid: 0x520F,
                    bus_id: "20".into(),
                    port_chain: vec![1, 4],
                    device_address: 9,
                    trigger_interface: Some(2),
                    product: None,
                },
            ],
        };
        let message = err.to_string();
        assert!(message.contains("Narrow the filter"), "{message}");
        assert!(message.contains("20-1.3"), "{message}");
        assert!(message.contains("20-1.4"), "{message}");
        assert!(message.contains("nRF52 USB"), "{message}");
    }
}
