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
extern crate chrono;

mod nanoext;
mod web;
mod temp;
mod tlog20;
mod shared;

use std::sync::RwLock;
use std::rc::Rc;
use std::env;

use log::{LogRecord, LogLevelFilter};
use env_logger::{LogBuilder, LogTarget};

use futures::Future;
use futures::unsync::mpsc;
use futures::unsync::oneshot;

use failure::Error;

use chrono::prelude::*;

use shared::SharedData;

pub const NANOEXT_SERIAL_DEVICE : &'static str = "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";
pub const TLOG20_SERIAL_DEVICE : &'static str = "/dev/serial/by-id/usb-FTDI_US232R_FT0IKONX-if00-port0";


pub enum ErrorEvent {
    NanoExtDecoderError
}

fn main() {
    match run() {
        Ok(()) => {},
        Err(e) => {
            error!("{:?}", e);
        }
    }
}

fn run() -> Result<(), Error> {

    init_logger();

    info!("START!");

    let shared = SharedData::new_rc();

    // any errors can be
    let (errorchannel_sender, _errorchannel_receiver) = mpsc::channel::<ErrorEvent>(1);
    let (_shutdown_trigger, shutdown_shot) = oneshot::channel::<()>();


    // Hyper will create the tokio core / reactor, which we will reuse for the serial ports
    let server = web::make_web_server(&shared);
    let reactor_handle = server.handle();

    nanoext::init_serial_port( &reactor_handle, &shared, errorchannel_sender.clone())?;

    tlog20::init_serial_port( &reactor_handle ).map_err(|e| {
        error!("{}", e);
    }).ok(); // ignore TLOG20 startup failure

    server.run_until(shutdown_shot.map_err(|_| ())).unwrap();

    Ok(())
}

fn init_logger() {

    let format = |record: &LogRecord| {
        let local: DateTime<Local> = Local::now();
        format!("{} - {} - {}", local.to_rfc2822(), record.level(), record.args())
    };

    let mut builder = LogBuilder::new();
    builder.target(LogTarget::Stdout);
    builder.format(format);
    builder.filter(None, LogLevelFilter::Info);
    if let Ok(log_spec) = env::var("RUST_LOG") {
        builder.parse(&log_spec);
    }
    builder.init().unwrap();
}