//! Windows transport (WinRT). Same GATT service as the macOS transport, plus a `discover`
//! helper that also lists classic RFCOMM services so we can see which link the buds offer
//! to Windows.
use crate::transport::Link;
use anyhow::{anyhow, bail, Result};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::core::{Ref, GUID, HSTRING};
use windows::Devices::Bluetooth::GenericAttributeProfile::*;
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::TypedEventHandler;
use windows::Devices::Bluetooth::Rfcomm::{RfcommDeviceService, RfcommServiceId};
use windows::Networking::Sockets::StreamSocket;
use windows::Storage::Streams::{DataReader, DataWriter, InputStreamOptions};
use std::sync::Mutex;

const SERVICE: GUID = GUID::from_u128(0x0000079a_d102_11e1_9b23_00025b00a5a5);
const WRITE_CHAR: GUID = GUID::from_u128(0x0100079a_d102_11e1_9b23_00025b00a5a5);
const NOTIFY_CHAR: GUID = GUID::from_u128(0x0200079a_d102_11e1_9b23_00025b00a5a5);

fn guid_str(g: &GUID) -> String {
    format!("{:?}", g).to_lowercase()
}

/// Everything a user needs to decide whether GATT or RFCOMM is available.
pub fn discover() -> Result<()> {
    println!("== Paired classic devices (RFCOMM services) ==");
    let sel = BluetoothDevice::GetDeviceSelectorFromPairingState(true)?;
    for info in DeviceInformation::FindAllAsyncAqsFilter(&sel)?.join()? {
        let name = info.Name()?.to_string();
        println!("{name}  [{}]", info.Id()?);
        let dev = match BluetoothDevice::FromIdAsync(&info.Id()?)?.join() {
            Ok(d) => d,
            Err(e) => {
                println!("  open failed: {e}");
                continue;
            }
        };
        println!("  connection: {:?}", dev.ConnectionStatus()?);
        match dev.GetRfcommServicesAsync() {
            Ok(op) => match op.join() {
                Ok(res) => {
                    println!("  rfcomm status: {:?}", res.Error()?);
                    for svc in res.Services()? {
                        println!("  rfcomm service {}", guid_str(&svc.ServiceId()?.Uuid()?));
                    }
                }
                Err(e) => println!("  rfcomm query failed: {e}"),
            },
            Err(e) => println!("  rfcomm query failed: {e}"),
        }
    }

    println!("\n== Paired BLE devices (GATT services) ==");
    let sel = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
    for info in DeviceInformation::FindAllAsyncAqsFilter(&sel)?.join()? {
        let name = info.Name()?.to_string();
        println!("{name}  [{}]", info.Id()?);
        let dev = match BluetoothLEDevice::FromIdAsync(&info.Id()?)?.join() {
            Ok(d) => d,
            Err(e) => {
                println!("  open failed: {e}");
                continue;
            }
        };
        let res = dev.GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached)?.join()?;
        println!("  gatt status: {:?}", res.Status()?);
        for svc in res.Services()? {
            let u = svc.Uuid()?;
            let mark = if u == SERVICE { "   <== buds control service" } else { "" };
            println!("  gatt service {}{mark}", guid_str(&u));
        }
    }

    println!("\n== Direct lookup of the buds GATT service ==");
    let sel = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE)?;
    let found = DeviceInformation::FindAllAsyncAqsFilter(&sel)?.join()?;
    println!("matches: {}", found.Size()?);
    for info in found {
        println!("  {}  [{}]", info.Name()?, info.Id()?);
    }
    Ok(())
}

use windows::Devices::Bluetooth::BluetoothCacheMode;

fn buffer_of(data: &[u8]) -> Result<windows::Storage::Streams::IBuffer> {
    let w = DataWriter::new()?;
    w.WriteBytes(data)?;
    Ok(w.DetachBuffer()?)
}

/// BLE GATT variant (kept as a fallback; the buds do not expose it to Windows when paired for audio).
fn connect_gatt(timeout: Duration) -> Result<Link> {
    let deadline = Instant::now() + timeout;
    let sel = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE)?;
    let found = DeviceInformation::FindAllAsyncAqsFilter(&sel)?.join()?;
    let info = found
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no paired device exposes the buds GATT service (run `buds discover`)"))?;
    let id: HSTRING = info.Id()?;
    let svc = GattDeviceService::FromIdAsync(&id)?.join()?;

    let get_char = |uuid: GUID| -> Result<GattCharacteristic> {
        let res = svc.GetCharacteristicsForUuidWithCacheModeAsync(uuid, BluetoothCacheMode::Uncached)?.join()?;
        if res.Status()? != GattCommunicationStatus::Success {
            bail!("characteristic discovery failed: {:?}", res.Status()?);
        }
        res.Characteristics()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("characteristic {} not found", guid_str(&uuid)))
    };
    let write_c = get_char(WRITE_CHAR)?;
    let notify_c = get_char(NOTIFY_CHAR)?;

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let handler = TypedEventHandler::new(move |_: Ref<GattCharacteristic>, args: Ref<GattValueChangedEventArgs>| {
        if let Some(args) = args.as_ref() {
            if let Ok(buf) = args.CharacteristicValue() {
                if let Ok(reader) = DataReader::FromBuffer(&buf) {
                    let mut v = vec![0u8; buf.Length().unwrap_or(0) as usize];
                    if reader.ReadBytes(&mut v).is_ok() {
                        let _ = tx.send(v);
                    }
                }
            }
        }
        Ok(())
    });
    let token = notify_c.ValueChanged(&handler)?;
    let st = notify_c
        .WriteClientCharacteristicConfigurationDescriptorAsync(GattClientCharacteristicConfigurationDescriptorValue::Notify)?
        .join()?;
    if st != GattCommunicationStatus::Success {
        bail!("subscribe failed: {st:?}");
    }
    if Instant::now() > deadline {
        bail!("timed out connecting to the buds");
    }

    // Keep the connection alive and the objects referenced for the lifetime of the Link.
    let session = svc
        .Device()
        .and_then(|d| d.BluetoothDeviceId())
        .and_then(|id| GattSession::FromDeviceIdAsync(&id)?.join())
        .ok();
    if let Some(s) = &session {
        let _ = s.SetMaintainConnection(true);
    }
    struct Keep(GattDeviceService, GattCharacteristic, i64, Option<GattSession>);
    // SAFETY: WinRT GATT objects are agile (free-threaded); we only call them from the write closure.
    unsafe impl Send for Keep {}
    unsafe impl Sync for Keep {}
    impl Drop for Keep {
        fn drop(&mut self) {
            let _ = self.1.RemoveValueChanged(self.2);
            if let Some(s) = &self.3 {
                let _ = s.SetMaintainConnection(false);
                let _ = s.Close();
            }
            let _ = self.0.Close();
        }
    }
    let keep = Keep(svc, notify_c, token, session);
    let write = Box::new(move |data: &[u8]| -> Result<()> {
        let _ = &keep; // owned by the closure: dropping the Link tears the connection down
        let buf = buffer_of(data)?;
        let st = write_c.WriteValueWithOptionAsync(&buf, GattWriteOption::WriteWithoutResponse)?.join()?;
        if st != GattCommunicationStatus::Success {
            bail!("write failed: {st:?}");
        }
        Ok(())
    });
    Ok(Link { write, rx })
}

/// Split an RFCOMM byte stream into frames: `AA <varint L> <L bytes>`.
fn take_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    while !buf.is_empty() && buf[0] != 0xAA {
        buf.remove(0);
    }
    let mut i = 1;
    let mut len = 0usize;
    let mut shift = 0;
    loop {
        let b = *buf.get(i)?;
        len |= ((b & 0x7f) as usize) << shift;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    let total = i + len;
    if buf.len() < total {
        return None;
    }
    Some(buf.drain(..total).collect())
}

/// Classic RFCOMM variant: what the Android app uses. The buds advertise the control
/// service under the same UUID as the GATT service.
fn connect_rfcomm(_timeout: Duration) -> Result<Link> {
    let sid = RfcommServiceId::FromUuid(SERVICE)?;
    let sel = RfcommDeviceService::GetDeviceSelector(&sid)?;
    let found = DeviceInformation::FindAllAsyncAqsFilter(&sel)?.join()?;
    let info = found
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no paired device offers the buds RFCOMM service (pair the buds in Windows Settings first)"))?;
    let svc = RfcommDeviceService::FromIdAsync(&info.Id()?)?.join()?;
    let socket = StreamSocket::new()?;
    socket.ConnectAsync(&svc.ConnectionHostName()?, &svc.ConnectionServiceName()?)?.join()?;

    let reader = DataReader::CreateDataReader(&socket.InputStream()?)?;
    reader.SetInputStreamOptions(InputStreamOptions::Partial)?;
    let writer = DataWriter::CreateDataWriter(&socket.OutputStream()?)?;

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let n = match reader.LoadAsync(1024).and_then(|op| op.join()) {
                Ok(0) | Err(_) => return, // closed: dropping tx makes rx report Disconnected
                Ok(n) => n as usize,
            };
            let mut chunk = vec![0u8; n];
            if reader.ReadBytes(&mut chunk).is_err() {
                return;
            }
            buf.extend(chunk);
            while let Some(f) = take_frame(&mut buf) {
                if tx.send(f).is_err() {
                    return;
                }
            }
        }
    });

    struct Keep(RfcommDeviceService, StreamSocket, Mutex<DataWriter>);
    // SAFETY: these WinRT objects are agile; access to the writer is serialised by the mutex.
    unsafe impl Send for Keep {}
    unsafe impl Sync for Keep {}
    impl Drop for Keep {
        fn drop(&mut self) {
            let _ = self.1.Close();
            let _ = self.0.Close();
        }
    }
    let keep = Keep(svc, socket, Mutex::new(writer));
    let write = Box::new(move |data: &[u8]| -> Result<()> {
        let w = keep.2.lock().map_err(|_| anyhow!("writer poisoned"))?;
        w.WriteBytes(data)?;
        w.StoreAsync()?.join()?;
        Ok(())
    });
    Ok(Link { write, rx })
}

/// Blocking: find the paired buds and open the control channel (RFCOMM, falling back to GATT).
pub fn connect(timeout: Duration) -> Result<Link> {
    match connect_rfcomm(timeout) {
        Ok(l) => Ok(l),
        Err(e) => connect_gatt(timeout).map_err(|g| anyhow!("RFCOMM: {e:#}; GATT: {g:#}")),
    }
}
