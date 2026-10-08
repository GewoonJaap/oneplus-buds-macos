//! CoreBluetooth transport (macOS). CONTRACT — implemented in cb.rs by a subagent.
//!
//! Buds that are already connected to this Mac are hidden from BLE scans, so the
//! implementation MUST use `retrieveConnectedPeripherals(withServices: [SERVICE])`.
use anyhow::Result;
use std::sync::mpsc::Receiver;
use std::time::Duration;

pub struct Link {
    pub(crate) write: Box<dyn Fn(&[u8]) -> Result<()> + Send + Sync>,
    /// Raw notification payloads from characteristic 0200079A.
    pub rx: Receiver<Vec<u8>>,
}

impl Link {
    /// Write to characteristic 0100079A using write-without-response.
    pub fn send(&self, data: &[u8]) -> Result<()> {
        (self.write)(data)
    }
}

/// Blocking: find the connected buds, connect, discover, subscribe to notifications.
pub fn connect(timeout: Duration) -> Result<Link> {
    #[cfg(target_os = "macos")]
    {
        crate::cb::connect(timeout)
    }
    #[cfg(windows)]
    {
        crate::win::connect(timeout)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = timeout;
        anyhow::bail!("only supported on macOS and Windows")
    }
}
