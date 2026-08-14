//! Shared device registry: pollers publish what they found, the API's
//! /devices endpoint reads it.

use crate::kasa::KasaDevice;
use crate::sensors::EnvDevice;
use crate::wiim::WiimDevice;
use std::sync::Mutex;

#[derive(Default)]
pub struct Registry {
    pub env: Mutex<Vec<EnvDevice>>,
    pub kasa: Mutex<Vec<KasaDevice>>,
    pub wiim: Mutex<Vec<WiimDevice>>,
    pub media_server: Mutex<Option<crate::ssdp::MediaServerLease>>,
}
