#![no_std]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(clippy::missing_safety_doc)]
#![deny(clippy::unnecessary_safety_doc)]
#![deny(clippy::unnecessary_safety_comment)]

#[cfg(test)]
extern crate std;


use embedded_io_async as eio;

#[cfg(all(feature = "log", feature = "defmt"))]
compile_error!("You may not enable both `log` and `defmt` features.");

mod fmt;
mod header;
mod io;
mod packet;

pub mod client;
pub mod config;
pub mod session;
pub mod types;

#[cfg(test)]
mod test;

#[cfg(feature = "v3")]
mod v3;
#[cfg(feature = "v5")]
mod v5;
