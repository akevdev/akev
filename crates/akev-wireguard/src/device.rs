//! The smoltcp "device" between the WireGuard tunnel and the TCP stack: two packet queues.
//! Decrypted IP packets from the peer are pushed into `rx`; packets smoltcp emits collect in
//! `tx` until the server loop encrypts and sends them.

use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use std::collections::VecDeque;

pub(crate) struct VirtualDevice {
    pub rx: VecDeque<Vec<u8>>,
    pub tx: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl VirtualDevice {
    pub(crate) fn new(mtu: usize) -> Self {
        VirtualDevice {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            mtu,
        }
    }
}

impl phy::Device for VirtualDevice {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((RxToken(packet), TxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

pub(crate) struct RxToken(Vec<u8>);

impl phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

pub(crate) struct TxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0u8; len];
        let r = f(&mut packet);
        self.0.push_back(packet);
        r
    }
}
