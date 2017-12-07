#![allow(unused_imports)]

extern crate hyper;
extern crate futures;
extern crate failure;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_serial;
extern crate tokio_signal;
extern crate bytes;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
extern crate env_logger;
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

use futures::future::{Future, ok, err};
use futures::Stream;
use futures::unsync::mpsc;
use futures::unsync::oneshot;
use futures::Sink;

use tokio_core::reactor::Handle;

use failure::Error;

use chrono::prelude::*;

use shared::{SharedData, SharedDataRc};

pub const NANOEXT_SERIAL_DEVICE : &'static str = "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";
pub const TLOG20_SERIAL_DEVICE : &'static str = "/dev/serial/by-id/usb-FTDI_US232R_FT0IKONX-if00-port0";
pub const NANOEXT_RESPAWN_COUNT : u32 = 5;

pub enum ErrorEvent {
    NanoExtDecoderError,
    CtrlC,
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
    let (errorchannel_sender, errorchannel_receiver) = mpsc::channel::<ErrorEvent>(1);
    let (shutdown_trigger, shutdown_shot) = oneshot::channel::<()>();


    // Hyper will create the tokio core / reactor, which we will reuse for the serial ports
    let server = web::make_web_server(&shared);
    let reactor_handle = server.handle();

    nanoext::init_serial_port( &reactor_handle, &shared, errorchannel_sender.clone())?;

    tlog20::init_serial_port( &reactor_handle ).map_err(|e| {
        error!("{}", e);
    }).ok(); // ignore TLOG20 startup failure


    // Handle errors in the Streams
    handle_errors(&reactor_handle,
                  shutdown_trigger,
                  &errorchannel_sender,
                  errorchannel_receiver,
                  &shared);


    // Handle Ctrl+c
    let ctrl_c = tokio_signal::ctrl_c(&reactor_handle).flatten_stream();

    // Process each ctrl-c as it comes in
    let mut errorchannel_sender2 = Some(errorchannel_sender.clone());
    let prog = ctrl_c.for_each(move |_| {
        errorchannel_sender2 = Some(
            errorchannel_sender2.take().unwrap()
                                .send(ErrorEvent::CtrlC).wait().unwrap());
        ok(())
    });

    reactor_handle.spawn(prog.map_err(|_| ()));

    server.run_until(shutdown_shot.map_err(|_| ())).unwrap();

    info!("SHUTDOWN!");

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

fn handle_errors(reactor_handle: &Handle,
                 shutdown_trigger : oneshot::Sender<()>,
                 errorchannel_sender : &mpsc::Sender<ErrorEvent>,
                 errorchannel_receiver : mpsc::Receiver<ErrorEvent>,
                 shared : &SharedDataRc) {

    let mut respawn_count = NANOEXT_RESPAWN_COUNT;

    // for moving into closure
    let reactor_handle2 = reactor_handle.clone();
    let errorchannel_sender = errorchannel_sender.clone();
    let shared = shared.clone();
    let mut shutdown_trigger = Some(shutdown_trigger); // because send consumes

    let error_handler = errorchannel_receiver.for_each(move |error| {

        match error {
            // A Error in the NanoExt. Try to respawn max 5 times.
            ErrorEvent::NanoExtDecoderError => {
                if respawn_count > 0 {
                    respawn_count -= 1;

                    let result = nanoext::init_serial_port( &reactor_handle2,
                                                            &shared,
                                                            errorchannel_sender.clone());
                    if result.is_err() {
                        error!("Could not respawn Nanoext: {}", result.unwrap_err());
                        shutdown_trigger.take().map(|st| st.send(()).unwrap() );
                    }
                } else {
                    error!("Respawned {} times. Shutdown now.", NANOEXT_RESPAWN_COUNT);
                    shutdown_trigger.take().map(|st| st.send(()).unwrap() );
                }
            },
            ErrorEvent::CtrlC => {
                info!("Ctrl-c received.");
                shutdown_trigger.take().map(|st| st.send(()).unwrap() );
            }
        }

        ok(())
    });

    reactor_handle.spawn(error_handler);
}