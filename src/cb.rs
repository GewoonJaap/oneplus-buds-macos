#![allow(non_snake_case)]
//! CoreBluetooth implementation of the transport (macOS, objc2).
//!
//! All CoreBluetooth objects live on a private serial dispatch queue. Delegate
//! callbacks run on that queue; `send()` hops onto it via `dispatch_async`.
use anyhow::{anyhow, bail, Result};
use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_core_bluetooth::{
    CBCentralManager, CBCentralManagerDelegate, CBCharacteristic, CBCharacteristicProperties,
    CBCharacteristicWriteType, CBManagerState, CBPeripheral, CBPeripheralDelegate, CBService,
    CBUUID,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSNumber, NSString};
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::transport::Link;

const SERVICE: &str = "0000079A-D102-11E1-9B23-00025B00A5A5";
const WRITE_CHAR: &str = "0100079A-D102-11E1-9B23-00025B00A5A5";
const NOTIFY_CHAR: &str = "0200079A-D102-11E1-9B23-00025B00A5A5";

#[link(name = "System", kind = "dylib")]
extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut AnyObject;
    fn dispatch_async(queue: *const AnyObject, block: &Block<dyn Fn()>);
}

/// Wrapper to move ObjC handles into shared state. They are only ever *used*
/// on the CB queue (or retained/released, which is thread-safe).
struct Handle<T>(Retained<T>);
unsafe impl<T> Send for Handle<T> {}
unsafe impl<T> Sync for Handle<T> {}

struct Shared {
    tx: Mutex<Option<Sender<Vec<u8>>>>,
    ready: Mutex<Option<Sender<std::result::Result<(), String>>>>,
    write_char: Mutex<Option<Handle<CBCharacteristic>>>,
    peripheral: Mutex<Option<Handle<CBPeripheral>>>,
    connected: AtomicBool,
    saw_peripheral: AtomicBool,
}

impl Shared {
    fn signal(&self, r: std::result::Result<(), String>) {
        if let Some(s) = self.ready.lock().unwrap().take() {
            let _ = s.send(r);
        }
    }
    fn fail(&self, msg: impl Into<String>) {
        self.signal(Err(msg.into()));
    }
}

fn uuid(s: &str) -> Retained<CBUUID> {
    unsafe { CBUUID::UUIDWithString(&NSString::from_str(s)) }
}

fn uuid_is(u: &CBUUID, s: &str) -> bool {
    unsafe { u.UUIDString() }.to_string().eq_ignore_ascii_case(s)
}

fn err_text(e: Option<&NSError>) -> String {
    match e {
        Some(e) => e.localizedDescription().to_string(),
        None => "unknown error".into(),
    }
}

declare_class!(
    struct Delegate;

    unsafe impl ClassType for Delegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "BudsCtlCbDelegate";
    }

    impl DeclaredClass for Delegate {
        type Ivars = Arc<Shared>;
    }

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl CBCentralManagerDelegate for Delegate {
        #[method(centralManagerDidUpdateState:)]
        unsafe fn centralManagerDidUpdateState(&self, central: &CBCentralManager) {
            let sh = self.ivars();
            let st = central.state();
            if st == CBManagerState::PoweredOn {
                let services = NSArray::from_vec(vec![uuid(SERVICE)]);
                let found = central.retrieveConnectedPeripheralsWithServices(&services);
                if let Some(p) = found.first_retained() {
                    self.begin_connect(central, p);
                } else {
                    // Fallback: scan for the service.
                    central.scanForPeripheralsWithServices_options(Some(&services), None);
                }
            } else if st == CBManagerState::Unauthorized {
                sh.fail("Bluetooth permission denied for this process (grant it in System Settings > Privacy & Security > Bluetooth)");
            } else if st == CBManagerState::Unsupported {
                sh.fail("Bluetooth LE unsupported on this Mac");
            } else if st == CBManagerState::PoweredOff {
                sh.fail("Bluetooth is turned off");
            }
        }

        #[method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:)]
        unsafe fn centralManager_didDiscoverPeripheral_advertisementData_RSSI(
            &self,
            central: &CBCentralManager,
            peripheral: &CBPeripheral,
            _ad: &NSDictionary<NSString, AnyObject>,
            _rssi: &NSNumber,
        ) {
            if self.ivars().peripheral.lock().unwrap().is_some() {
                return;
            }
            central.stopScan();
            self.begin_connect(central, peripheral.retain());
        }

        #[method(centralManager:didConnectPeripheral:)]
        unsafe fn centralManager_didConnectPeripheral(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
        ) {
            self.ivars().connected.store(true, Ordering::SeqCst);
            let services = NSArray::from_vec(vec![uuid(SERVICE)]);
            peripheral.discoverServices(Some(&services));
        }

        #[method(centralManager:didFailToConnectPeripheral:error:)]
        unsafe fn centralManager_didFailToConnectPeripheral_error(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            self.ivars().fail(format!("failed to connect to buds: {}", err_text(error)));
        }

        #[method(centralManager:didDisconnectPeripheral:error:)]
        unsafe fn centralManager_didDisconnectPeripheral_error(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            _error: Option<&NSError>,
        ) {
            let sh = self.ivars();
            sh.connected.store(false, Ordering::SeqCst);
            // Dropping the sender makes `rx.recv()` return Err.
            sh.tx.lock().unwrap().take();
            sh.fail("buds disconnected during setup");
        }
    }

    unsafe impl CBPeripheralDelegate for Delegate {
        #[method(peripheral:didDiscoverServices:)]
        unsafe fn peripheral_didDiscoverServices(
            &self,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            if error.is_some() {
                self.ivars().fail(format!("service discovery failed: {}", err_text(error)));
                return;
            }
            let svc = peripheral.services().and_then(|s| {
                s.iter()
                    .find(|x| uuid_is(&x.UUID(), SERVICE))
                    .map(|x| x.retain())
            });
            match svc {
                Some(s) => {
                    let chars = NSArray::from_vec(vec![uuid(WRITE_CHAR), uuid(NOTIFY_CHAR)]);
                    peripheral.discoverCharacteristics_forService(Some(&chars), &s);
                }
                None => self.ivars().fail("buds do not expose GATT service 079A"),
            }
        }

        #[method(peripheral:didDiscoverCharacteristicsForService:error:)]
        unsafe fn peripheral_didDiscoverCharacteristicsForService_error(
            &self,
            peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&NSError>,
        ) {
            let sh = self.ivars();
            if error.is_some() {
                sh.fail(format!("characteristic discovery failed: {}", err_text(error)));
                return;
            }
            let mut have_notify = false;
            let mut have_write = false;
            if let Some(chars) = service.characteristics() {
                for c in chars.iter().map(|c| c.retain()) {
                    let u = c.UUID();
                    if uuid_is(&u, NOTIFY_CHAR) {
                        if c.properties().contains(CBCharacteristicProperties::CBCharacteristicPropertyNotify) {
                            peripheral.setNotifyValue_forCharacteristic(true, &c);
                            have_notify = true;
                        }
                    } else if uuid_is(&u, WRITE_CHAR) {
                        let p = c.properties();
                        if p.contains(CBCharacteristicProperties::CBCharacteristicPropertyWriteWithoutResponse)
                            || p.contains(CBCharacteristicProperties::CBCharacteristicPropertyWrite)
                        {
                            *sh.write_char.lock().unwrap() = Some(Handle(c));
                            have_write = true;
                        }
                    }
                }
            }
            if !have_notify || !have_write {
                sh.fail("buds GATT service is missing the expected write/notify characteristics");
            }
        }

        #[method(peripheral:didUpdateNotificationStateForCharacteristic:error:)]
        unsafe fn peripheral_didUpdateNotificationStateForCharacteristic_error(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            if !uuid_is(&characteristic.UUID(), NOTIFY_CHAR) {
                return;
            }
            if error.is_some() {
                self.ivars().fail(format!("enabling notifications failed: {}", err_text(error)));
            } else if characteristic.isNotifying() {
                self.ivars().signal(Ok(()));
            }
        }

        #[method(peripheral:didUpdateValueForCharacteristic:error:)]
        unsafe fn peripheral_didUpdateValueForCharacteristic_error(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            if error.is_some() || !uuid_is(&characteristic.UUID(), NOTIFY_CHAR) {
                return;
            }
            if let Some(v) = characteristic.value() {
                if let Some(tx) = self.ivars().tx.lock().unwrap().as_ref() {
                    let _ = tx.send(v.bytes().to_vec());
                }
            }
        }
    }
);

impl Delegate {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(shared);
        unsafe { msg_send_id![super(this), init] }
    }

    unsafe fn begin_connect(&self, central: &CBCentralManager, p: Retained<CBPeripheral>) {
        let sh = self.ivars();
        sh.saw_peripheral.store(true, Ordering::SeqCst);
        p.setDelegate(Some(ProtocolObject::from_ref(self)));
        *sh.peripheral.lock().unwrap() = Some(Handle(p.clone()));
        central.connectPeripheral_options(&p, None);
    }
}

/// Everything that must stay alive for the lifetime of the Link.
struct Keepalive {
    shared: Arc<Shared>,
    delegate: Retained<Delegate>,
    manager: Retained<CBCentralManager>,
    queue: Retained<AnyObject>,
}
unsafe impl Send for Keepalive {}
unsafe impl Sync for Keepalive {}

impl Keepalive {
    /// Run `f` asynchronously on the CB queue.
    fn run_on_queue(&self, f: impl Fn() + 'static) {
        let block = RcBlock::new(f);
        unsafe { dispatch_async(&*self.queue, &block) };
    }
}

impl Drop for Keepalive {
    fn drop(&mut self) {
        // Cancel the GATT connection on the CB queue; the block keeps the
        // objects alive until it has run.
        let manager = Handle(self.manager.clone());
        let delegate = Handle(self.delegate.clone());
        let shared = self.shared.clone();
        self.run_on_queue(move || {
            unsafe {
                if let Some(p) = shared.peripheral.lock().unwrap().take() {
                    manager.0.cancelPeripheralConnection(&p.0);
                    p.0.setDelegate(None);
                }
                manager.0.stopScan();
            }
            shared.write_char.lock().unwrap().take();
            let _ = &delegate;
        });
    }
}

pub fn connect(timeout: Duration) -> Result<Link> {
    let (tx, rx) = channel::<Vec<u8>>();
    let (rdy_tx, rdy_rx) = channel::<std::result::Result<(), String>>();
    let shared = Arc::new(Shared {
        tx: Mutex::new(Some(tx)),
        ready: Mutex::new(Some(rdy_tx)),
        write_char: Mutex::new(None),
        peripheral: Mutex::new(None),
        connected: AtomicBool::new(false),
        saw_peripheral: AtomicBool::new(false),
    });
    let delegate = Delegate::new(shared.clone());

    // Own serial queue (NULL attr == DISPATCH_QUEUE_SERIAL).
    let queue = unsafe {
        let q = dispatch_queue_create(c"buds-ctl.cb".as_ptr(), std::ptr::null());
        Retained::from_raw(q).ok_or_else(|| anyhow!("dispatch_queue_create failed"))?
    };
    let manager: Retained<CBCentralManager> = unsafe {
        let d: &ProtocolObject<dyn CBCentralManagerDelegate> = ProtocolObject::from_ref(&*delegate);
        msg_send_id![
            CBCentralManager::alloc(),
            initWithDelegate: Some(d),
            queue: &*queue
        ]
    };

    let keep = Keepalive { shared: shared.clone(), delegate, manager, queue };

    match rdy_rx.recv_timeout(timeout) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => bail!("{e}"),
        Err(_) => {
            if !shared.saw_peripheral.load(Ordering::SeqCst) {
                bail!("buds not connected to this Mac (no peripheral with service 079A found within {timeout:?})");
            }
            bail!("timed out talking to the buds over Bluetooth LE after {timeout:?}");
        }
    }
    // `keep` dropped on error paths above cancels any pending connection.

    let write = move |data: &[u8]| -> Result<()> {
        if !keep.shared.connected.load(Ordering::SeqCst) {
            bail!("buds disconnected");
        }
        let data = data.to_vec();
        let shared = keep.shared.clone();
        let peripheral = shared.peripheral.lock().unwrap().as_ref().map(|p| Handle(p.0.clone()));
        let ch = shared.write_char.lock().unwrap().as_ref().map(|c| Handle(c.0.clone()));
        let (Some(p), Some(c)) = (peripheral, ch) else { bail!("not connected") };
        keep.run_on_queue(move || unsafe {
            let d = NSData::with_bytes(&data);
            p.0.writeValue_forCharacteristic_type(&d, &c.0, CBCharacteristicWriteType::CBCharacteristicWriteWithoutResponse);
        });
        Ok(())
    };

    Ok(Link { write: Box::new(write), rx })
}
