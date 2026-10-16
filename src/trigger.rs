use std::thread::sleep;
use std::time::{Duration, Instant};

use nusb::MaybeFuture;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};

use crate::discovery::{self, DeviceMatch, Filter};
use crate::error::Error;

const DFU_DETACH_REQUEST: u8 = 0x00;
const NORDIC_SEM_VER_REQUEST: u8 = 0x08;
const DFU_DETACH_PAYLOAD: &[u8] = b"0";

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(1);

/// Options for [`reset`].
#[derive(Clone, Debug)]
pub struct ResetOptions {
    /// How long to wait for the device to re-enumerate after the detach.
    ///
    /// Default 5 s.
    pub timeout: Duration,

    /// How often to poll the bus while waiting.
    ///
    /// Default 100 ms.
    pub poll_interval: Duration,
}

impl Default for ResetOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(100),
        }
    }
}

/// The result of a successful, verified reset.
#[derive(Clone, Debug)]
pub struct ResetOutcome {
    /// The device that was reset, as it appeared in application mode.
    pub device: DeviceMatch,

    /// Application firmware version read from the trigger interface, if available.
    pub firmware_version: Option<String>,

    /// Vendor ID the device re-enumerated with (DFU mode).
    pub dfu_vid: u16,

    /// Product ID the device re-enumerated with (DFU mode).
    pub dfu_pid: u16,
}

/// Reset the uniquely matching device into DFU mode and verify it re-enumerated.
///
/// Requires exactly one device matching `filter` that exposes the DFU trigger
/// interface. Otherwise it returns [`Error::NoDevice`] or [`Error::MultipleDevices`]
/// without contacting any device. The device disconnecting mid-detach is treated
/// as success. Verification waits for a device to re-appear at the same
/// physical USB port with a new identity (address or VID:PID).
pub fn reset(filter: Filter, options: &ResetOptions) -> Result<ResetOutcome, Error> {
    let (info, target) = select_unique(discovery::list_with_info(filter)?)?;
    let interface_number = target
        .trigger_interface
        .expect("select_unique only returns trigger-capable devices");

    log::info!("resetting {target} into DFU mode");
    let device = info.open().wait()?;
    let interface = device.detach_and_claim_interface(interface_number).wait()?;

    let firmware_version = read_firmware_version(&interface, interface_number);
    if let Some(version) = &firmware_version {
        log::info!("application firmware version: {version}");
    }

    let detach = interface.control_out(
        ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: DFU_DETACH_REQUEST,
            value: 0,
            index: u16::from(interface_number),
            data: DFU_DETACH_PAYLOAD,
        },
        TRANSFER_TIMEOUT,
    );
    detach_outcome(detach.wait())?;
    drop(interface);
    drop(device);
    log::info!(
        "DFU detach sent. Waiting up to {:?} for re-enumeration at port {}",
        options.timeout,
        target.port_path()
    );

    let (dfu_vid, dfu_pid) = wait_for_reenumeration(
        enumerate_observations,
        &target,
        options.timeout,
        options.poll_interval,
    )?;
    log::info!("device re-enumerated as {dfu_vid:04x}:{dfu_pid:04x}");

    Ok(ResetOutcome {
        device: target,
        firmware_version,
        dfu_vid,
        dfu_pid,
    })
}

/// Pick the single trigger-capable device from the filter matches.
pub(crate) fn select_unique<D>(matches: Vec<(D, DeviceMatch)>) -> Result<(D, DeviceMatch), Error> {
    let mut capable: Vec<(D, DeviceMatch)> = matches
        .into_iter()
        .filter(|(_, device)| device.has_trigger_interface())
        .collect();
    match capable.len() {
        0 => Err(Error::NoDevice),
        1 => Ok(capable.remove(0)),
        _ => Err(Error::MultipleDevices {
            candidates: capable.into_iter().map(|(_, device)| device).collect(),
        }),
    }
}

/// Classify the detach transfer result.
///
/// The device reboots mid-request, so a stall or disconnect error is the
/// expected success signal, mirroring nrfutil's `LIBUSB_ERROR_PIPE` handling.
pub(crate) fn detach_outcome(result: Result<(), TransferError>) -> Result<(), TransferError> {
    match result {
        Ok(()) => Ok(()),
        Err(TransferError::Stall | TransferError::Disconnected) => {
            log::debug!("detach transfer ended with the expected stall or disconnect error");
            Ok(())
        }
        Err(other) => Err(other),
    }
}

fn read_firmware_version(interface: &nusb::Interface, interface_number: u8) -> Option<String> {
    let result = interface
        .control_in(
            ControlIn {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: NORDIC_SEM_VER_REQUEST,
                value: 0,
                index: u16::from(interface_number),
                length: 255,
            },
            TRANSFER_TIMEOUT,
        )
        .wait();
    firmware_version_from_result(result)
}

/// Decode the semver response.
///
/// Any failure is logged and reported as `None`, so a missing version never
/// aborts the reset.
pub(crate) fn firmware_version_from_result(
    result: Result<Vec<u8>, TransferError>,
) -> Option<String> {
    match result {
        Ok(bytes) => {
            let version = String::from_utf8_lossy(&bytes);
            let version = version.trim_end_matches('\0').trim();
            if version.is_empty() {
                log::warn!("DFU trigger interface returned an empty firmware version");
                None
            } else {
                Some(version.to_string())
            }
        }
        Err(error) => {
            log::warn!("could not read application firmware version: {error}");
            None
        }
    }
}

/// A device observed on the bus, reduced to what re-enumeration tracking needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Observation {
    pub bus_id: String,
    pub port_chain: Vec<u8>,
    pub device_address: u8,
    pub vid: u16,
    pub pid: u16,
}

fn enumerate_observations() -> Vec<Observation> {
    match nusb::list_devices().wait() {
        Ok(devices) => devices
            .map(|info| Observation {
                bus_id: info.bus_id().to_string(),
                port_chain: info.port_chain().to_vec(),
                device_address: info.device_address(),
                vid: info.vendor_id(),
                pid: info.product_id(),
            })
            .collect(),
        Err(error) => {
            log::debug!("enumeration failed while polling for re-enumeration: {error}");
            Vec::new()
        }
    }
}

/// Wait until a device with a new identity appears at `target`'s port.
///
/// "New identity" means a different bus address or VID:PID than the device had
/// in application mode, so the still-enumerated original device does not count.
pub(crate) fn wait_for_reenumeration(
    mut enumerate: impl FnMut() -> Vec<Observation>,
    target: &DeviceMatch,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(u16, u16), Error> {
    let start = Instant::now();
    loop {
        let observations = enumerate();
        if let Some(found) = observations.iter().find(|o| {
            o.bus_id == target.bus_id
                && o.port_chain == target.port_chain
                && (o.device_address != target.device_address
                    || o.vid != target.vid
                    || o.pid != target.pid)
        }) {
            return Ok((found.vid, found.pid));
        }
        if start.elapsed() >= timeout {
            return Err(Error::TriggerSentButNotVerified { waited: timeout });
        }
        sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(port: u8, trigger: bool) -> DeviceMatch {
        DeviceMatch {
            vid: 0x1915,
            pid: 0x520F,
            bus_id: "20".into(),
            port_chain: vec![1, port],
            device_address: 5,
            trigger_interface: trigger.then_some(2),
            serial: None,
            product: None,
        }
    }

    #[test]
    fn select_unique_picks_single_trigger_capable_device() {
        let matches = vec![((), device(1, false)), ((), device(2, true))];
        let (_, selected) = select_unique(matches).unwrap();
        assert_eq!(selected.port_chain, vec![1, 2]);
    }

    #[test]
    fn select_unique_errors_when_nothing_is_trigger_capable() {
        let matches = vec![((), device(1, false)), ((), device(2, false))];
        assert!(matches!(select_unique(matches), Err(Error::NoDevice)));
        assert!(matches!(select_unique::<()>(vec![]), Err(Error::NoDevice)));
    }

    #[test]
    fn select_unique_errors_on_ambiguity_and_reports_candidates() {
        let matches = vec![((), device(1, true)), ((), device(2, true))];
        match select_unique(matches) {
            Err(Error::MultipleDevices { candidates }) => assert_eq!(candidates.len(), 2),
            other => panic!("expected MultipleDevices, got {other:?}"),
        }
    }

    #[test]
    fn detach_stall_and_disconnect_are_success() {
        assert!(detach_outcome(Ok(())).is_ok());
        assert!(detach_outcome(Err(TransferError::Stall)).is_ok());
        assert!(detach_outcome(Err(TransferError::Disconnected)).is_ok());
    }

    #[test]
    fn detach_other_errors_are_failures() {
        assert!(detach_outcome(Err(TransferError::Cancelled)).is_err());
        assert!(detach_outcome(Err(TransferError::Fault)).is_err());
    }

    #[test]
    fn firmware_version_decodes_and_trims() {
        let bytes = b"blinky 1.2.3\0\0".to_vec();
        assert_eq!(
            firmware_version_from_result(Ok(bytes)).as_deref(),
            Some("blinky 1.2.3")
        );
    }

    #[test]
    fn firmware_version_read_failure_is_none_not_panic() {
        assert_eq!(
            firmware_version_from_result(Err(TransferError::Stall)),
            None
        );
        assert_eq!(firmware_version_from_result(Ok(Vec::new())), None);
    }

    fn observation(port: u8, address: u8, vid: u16, pid: u16) -> Observation {
        Observation {
            bus_id: "20".into(),
            port_chain: vec![1, port],
            device_address: address,
            vid,
            pid,
        }
    }

    #[test]
    fn reenumeration_detected_when_new_identity_appears_at_port() {
        let target = device(3, true);
        let mut calls = 0;
        let result = wait_for_reenumeration(
            || {
                calls += 1;
                match calls {
                    // Old device still present, then gone, then bootloader appears.
                    1 => vec![observation(3, 5, 0x1915, 0x520F)],
                    2 => vec![],
                    _ => vec![observation(3, 6, 0x1915, 0x521F)],
                }
            },
            &target,
            Duration::from_secs(1),
            Duration::from_millis(1),
        );
        assert_eq!(result.unwrap(), (0x1915, 0x521F));
    }

    #[test]
    fn reenumeration_ignores_other_ports_and_times_out() {
        let target = device(3, true);
        let result = wait_for_reenumeration(
            // A different port re-enumerating must not count.
            || vec![observation(4, 9, 0x1915, 0x521F)],
            &target,
            Duration::from_millis(20),
            Duration::from_millis(1),
        );
        assert!(matches!(
            result,
            Err(Error::TriggerSentButNotVerified { .. })
        ));
    }

    #[test]
    fn reenumeration_same_identity_does_not_count() {
        let target = device(3, true);
        let result = wait_for_reenumeration(
            // The original device, unchanged (same address and VID:PID).
            || vec![observation(3, 5, 0x1915, 0x520F)],
            &target,
            Duration::from_millis(20),
            Duration::from_millis(1),
        );
        assert!(matches!(
            result,
            Err(Error::TriggerSentButNotVerified { .. })
        ));
    }
}
