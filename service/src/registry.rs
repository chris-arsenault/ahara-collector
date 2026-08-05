//! Shared device registry: pollers publish what they found, the API's
//! /devices endpoint reads it.

use crate::kasa::KasaDevice;
use crate::sensors::EnvDevice;
use std::sync::Mutex;

#[derive(Default)]
pub struct Registry {
    pub env: Mutex<Vec<EnvDevice>>,
    pub kasa: Mutex<Vec<KasaDevice>>,
}
