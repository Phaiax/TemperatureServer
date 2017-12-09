#![allow(unused_imports)]

extern crate bytes;
extern crate chrono;
extern crate env_logger;
extern crate failure;
extern crate futures;
extern crate hyper;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_serial;
extern crate tokio_signal;

mod nanoext;
mod web;
mod temp;
mod tlog20;
mod shared;

use std::sync::RwLock;
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::env;

use log::{LogLevelFilter, LogRecord};
use env_logger::{LogBuilder, LogTarget};

use futures::{future, Future};
use futures::Stream;
use futures::unsync::mpsc;
use futures::unsync::oneshot;
use futures::Sink;

use tokio_core::reactor::Handle;

use failure::Error;

use chrono::prelude::*;

use nanoext::{NanoExtCommand, NanoextCommandSink};
use temp::TemperatureStats;

use shared::{Shared, SharedInner, setup_shared};

pub const NANOEXT_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";
pub const TLOG20_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-FTDI_US232R_FT0IKONX-if00-port0";
pub const NANOEXT_RESPAWN_COUNT: u32 = 5;

pub const STDIN_BUFFER_SIZE: usize = 1000;


pub enum Event {
    NanoExtDecoderError,
    CtrlC,
}


fn main() {
    // Init logging
    init_logger();

    match run() {
        Ok(()) => {}
        Err(e) => {
            error!("{:?}", e);
        }
    }
}

fn run() -> Result<(), Error> {

    // Oneshot to exit event loop
    let (shutdown_trigger, shutdown_shot) = setup_shutdown_oneshot();

    // Channel to the high-level event loop (see `handle_events()`)
    let (event_sink, event_stream) = mpsc::channel::<Event>(1);

    // Setup shared data (Handle and CommandSink still missing)
    let shared = setup_shared(event_sink);

    // Hyper will create the tokio core / reactor ...
    let server = web::make_web_server(&shared);

    // ... which we will reuse for the serial ports and everything else
    shared.put_handle(server.handle());

    // Now connect to the Arduino (called the NanoExt)
    let command_sink = nanoext::init_serial_port(shared.clone())?;
    shared.put_command_sink(command_sink, shared.clone());

    // Now connect to the TLOG20, but do not fail if not present
    tlog20::init_serial_port(shared.clone())
        .map_err(|e| error!("{}", e) ) .ok();

    // Handle SIG_INT
    setup_ctrlc_forwarding(shared.clone());

    // Handle stdin. Command interpreting occours here.
    setup_stdin_handling(shared.clone());

    // Handle events, including error events and do restart serial port.
    // Events can be sent via `shared.handle_event_async()`
    handle_events(shared.clone(), event_stream, shutdown_trigger);

    info!("START EVENT LOOP!");

    // Start Webserver and block until shutdown is triggered
    server.run_until(shutdown_shot.map_err(|_| ())).unwrap();

    info!("SHUTDOWN!");

    Ok(())
}

fn init_logger() {
    let format = |record: &LogRecord| {
        let local: DateTime<Local> = Local::now();
        format!(
            "{} - {} - {}",
            local.to_rfc2822(),
            record.level(),
            record.args()
        )
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

type ShutdownTrigger = Box<FnMut() -> ()>;
type ShutdownShot = oneshot::Receiver<()>;

fn setup_shutdown_oneshot() -> (ShutdownTrigger, ShutdownShot) {
    let (shutdown_trigger, shutdown_shot) = oneshot::channel::<()>();
    // wrap trigger in closure that abstracts the consuming behaviour of send
    let mut shutdown_trigger = Some(shutdown_trigger);
    let shutdown_trigger = Box::new(move || {
        shutdown_trigger.take().map(|s_t| s_t.send(()).ok() /* no future involved here */ );
    });
    (shutdown_trigger, shutdown_shot)
}

/// Events can be sent via `shared.handle_event_async()`
///
/// The error handler will listen on the errorchannel and
/// will restart the serial port if there was an error
/// or it will gracefully quit the program when ctrl+c is pressed.
fn handle_events(
    shared: Shared,
    error_stream: mpsc::Receiver<Event>,
    mut shutdown_trigger: ShutdownTrigger,
) {
    let mut respawn_count: u32 = NANOEXT_RESPAWN_COUNT;

    // for moving into closure
    let shared1 = shared.clone();

    let error_handler = error_stream.for_each(move |error| {
        match error {
            // A Error in the NanoExt. Try to respawn max 5 times.
            Event::NanoExtDecoderError => if respawn_count == 0 {
                error!("Respawned {} times. Shutdown now.", NANOEXT_RESPAWN_COUNT);
                shutdown_trigger();
            } else
            /* if respawn_count > 0 */
            {
                respawn_count -= 1;

                match nanoext::init_serial_port(shared1.clone()) {
                    Ok(command_sink) => {
                        shared1.put_command_sink(command_sink, shared1.clone());
                    }
                    Err(err) => {
                        error!("Could not respawn Nanoext: {}", err);
                        shutdown_trigger();
                    }
                }
            },
            Event::CtrlC => {
                info!("Ctrl-c received.");
                shutdown_trigger();
            }
        }

        future::ok(())
    });

    shared.spawn(error_handler);
}


/// Handle Ctrl+c aka SIG_INT
fn setup_ctrlc_forwarding(shared: Shared) {
    let ctrl_c = tokio_signal::ctrl_c(&shared.handle()).flatten_stream();

    let shared1 = shared.clone();

    // Process each ctrl-c as it comes in
    let prog = ctrl_c.for_each(move |_| {
        shared1.handle_event_async(Event::CtrlC);
        future::ok(())
    });

    shared.spawn(prog.map_err(|_| ()));
}


fn setup_stdin_handling(shared: Shared) {

    let stdin_stream = tokio_stdin::spawn_stdin_stream();

    let shared1 = shared.clone();

    let prog = stdin_stream.for_each(move |line| {
        match line.as_str() {
            "rc" => {
                info!("Strong count of shared: {}", ::std::rc::Rc::strong_count(&shared1));
            },
            "1" => shared1.send_command_async(NanoExtCommand::PowerOn, shared1.clone()),
            "0" => shared1.send_command_async(NanoExtCommand::PowerOff, shared1.clone()),
            _ => {},
        }
        future::ok(())
    }).map_err(|e| {
        debug!("StdIn canceld: {:?}", e);
        ()
    });

    shared.spawn(prog);
}

// from crate `tokio-stdin`
// But buffer lines instead of sending each byte by itself
mod tokio_stdin {

    use futures::stream::iter_result;
    use futures::{Future, Sink, Stream};
    use futures::sync::mpsc::{Receiver, SendError, channel};
    use std::io::{self, BufRead};
    use std::thread;

    pub type Line = String;
    pub type StdInStream = Receiver<Line>;

    #[derive(Debug)]
    pub enum StdinError {
        Stdin(::std::io::Error),
        Channel(SendError<Line>),
    }

    pub fn spawn_stdin_stream() -> StdInStream {

        let (channel_sink, channel_stream) = channel(super::STDIN_BUFFER_SIZE);

        let stdin_sink = channel_sink.sink_map_err(StdinError::Channel);

        thread::spawn(move || {
            let stdin = io::stdin();
            let stdin_lock = stdin.lock();
            // In contrast to iter_ok, iter_result will make the receiver poll an Err() if lines()
            // retured an err.
            iter_result(stdin_lock.lines())
                .map_err(StdinError::Stdin)
                .forward(stdin_sink)
                .wait()
                .unwrap();
        });

        channel_stream
    }
}