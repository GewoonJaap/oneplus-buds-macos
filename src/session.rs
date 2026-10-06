//! Handshake + request helpers on top of a transport `Link`.
use crate::protocol::*;
use crate::transport::{self, Link};
use anyhow::{anyhow, Result};
use std::time::{Duration, Instant};

pub struct Session {
    pub link: Link,
    pub b: Builder,
}

const STEP: Duration = Duration::from_millis(1600);

impl Session {
    pub fn open() -> Result<Self> {
        let link = transport::connect(Duration::from_secs(10))?;
        let mut s = Session { link, b: Builder::new() };
        let f = s.b.hello();
        s.link.send(&f)?;
        std::thread::sleep(Duration::from_millis(2000));
        let f = s.b.register(DEFAULT_TOKEN);
        s.link.send(&f)?;
        std::thread::sleep(STEP);
        Ok(s)
    }

    fn wait_for<T>(&self, timeout: Duration, mut f: impl FnMut(Event) -> Option<T>) -> Result<T> {
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let raw = self.link.rx.recv_timeout(left).map_err(|_| anyhow!("timed out waiting for the buds"))?;
            if let Some(v) = decode(&raw).and_then(&mut f) {
                return Ok(v);
            }
        }
    }

    pub fn query_mode(&mut self) -> Result<Option<Mode>> {
        let f = self.b.noise_query();
        self.link.send(&f)?;
        self.wait_for(Duration::from_secs(5), |e| match e {
            Event::Noise(m, _) => Some(m),
            _ => None,
        })
    }

    pub fn query_battery(&mut self) -> Result<Battery> {
        let f = self.b.battery_query();
        self.link.send(&f)?;
        self.wait_for(Duration::from_secs(5), |e| match e {
            Event::Battery(b) => Some(b),
            _ => None,
        })
    }

    pub fn set_mode(&mut self, mode: Mode) -> Result<Option<Mode>> {
        let f = self.b.noise_set(mode);
        self.link.send(&f)?;
        std::thread::sleep(Duration::from_millis(800));
        self.query_mode()
    }

    pub fn query_eq(&mut self) -> Result<Option<Eq>> {
        let f = self.b.eq_query();
        self.link.send(&f)?;
        self.wait_for(Duration::from_secs(5), |e| match e {
            Event::Other(0x810F, p) if p.len() >= 2 => Some(Eq::from_id(p[1])),
            _ => None,
        })
    }

    pub fn set_eq(&mut self, e: Eq) -> Result<Option<Eq>> {
        let f = self.b.eq_set(e);
        self.link.send(&f)?;
        std::thread::sleep(Duration::from_millis(500));
        self.query_eq()
    }

    pub fn set_spatial(&mut self, m: Spatial) -> Result<()> {
        let f = self.b.spatial_set(m);
        self.link.send(&f)?;
        self.wait_for(Duration::from_secs(3), |e| match e {
            Event::Other(0x8422, p) if p.first() == Some(&0) => Some(()),
            _ => None,
        })
    }

    pub fn query_spatial(&mut self) -> Result<Option<Spatial>> {
        let f = self.b.raw(0x2A, 0x01, &[]);
        self.link.send(&f)?;
        self.wait_for(Duration::from_secs(5), |e| match e {
            Event::Other(0x812A, p) if p.len() >= 2 => Some(Spatial::ALL.into_iter().find(|m| *m as u8 == p[1])),
            _ => None,
        })
    }
}
