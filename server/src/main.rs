#![allow(unused_imports)]

extern crate hyper;
extern crate futures;
extern crate failure;
extern crate tokio_serial;
extern crate tokio_io;
extern crate bytes;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
extern crate env_logger;
extern crate tokio_core;

mod nanoext;
mod web;
mod temp;

use std::sync::RwLock;
use std::cell::Cell;
use std::rc::Rc;
use std::env;

use log::LogRecord;
use env_logger::{LogBuilder, LogTarget};

use futures::future::{Future, ok, err};

use temp::TemperatureStats;

use failure::Error;


pub const NANOEXT_SERIAL_DEVICE : &'static str = "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";

type SharedData = Rc<Cell<TemperatureStats>>;



fn main() {

    init_logger();

    let shared : SharedData = Rc::new(Cell::new(TemperatureStats::default()));

    let server = web::make_web_server(&shared);

    nanoext::init_serial_port( server.handle(), &shared );

    server.run().unwrap();
}

fn init_logger() {

    let format = |record: &LogRecord| {
        format!("{} - {}", record.level(), record.args())
    };

    let mut builder = LogBuilder::new();
    builder.target(LogTarget::Stdout);
    builder.format(format);
    if let Ok(logdirectory) = env::var("RUST_LOG") {
        builder.parse(&logdirectory);
    }
    builder.init().unwrap();
}