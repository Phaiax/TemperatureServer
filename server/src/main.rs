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

pub const NANOEXT_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";
pub const TLOG20_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-FTDI_US232R_FT0IKONX-if00-port0";
pub const NANOEXT_RESPAWN_COUNT: u32 = 5;

pub const STDIN_BUFFER_SIZE: usize = 1000;


pub enum ErrorEvent {
    NanoExtDecoderError,
    CtrlC,
}


pub type Shared = Rc<SharedInner>;

pub struct SharedInner {
    pub temperatures: Cell<TemperatureStats>,
    error_sink: mpsc::Sender<ErrorEvent>,
    pending_nanoext_command : RefCell<Option<NanoExtCommand>>,
    nanoext_command_sink: RefCell<Option<NanoextCommandSink>>,
    reactor_handle: RefCell<Option<Handle>>,
}

impl SharedInner {
    pub fn handle_error_async(&self, e: ErrorEvent) {
        let send_and_flush = self.error_sink
            .clone()
            .send(e)
            .map(|_error_sink| ())
            .map_err(|_e| {
                error!("Could not handle error in an async fashion.");
                ()
            });
        self.spawn(send_and_flush);
    }

    pub fn handle(&self) -> Handle {
        self.reactor_handle.borrow()
            .as_ref()
            .expect("Called handle() before handle was ready.")
            .clone()
    }

    pub fn spawn<F>(&self, f: F)
    where
        F: Future<Item = (), Error = ()> + 'static,
    {
        self.reactor_handle.borrow()
            .as_ref()
            .expect("Called handle() before handle was ready.")
            .spawn(f);
    }

    fn put_handle(&self, handle: Handle) {
        let mut option = self.reactor_handle.borrow_mut();
        if option.is_some() {
            panic!("Attempt to put reactor_handle into Shared twice.");
        }
        ::std::mem::replace(&mut *option, Some(handle));
    }

    /// If a new serial connection is created, put the corresponding sink here,
    /// so we have a place where to dispatch commands
    fn put_command_sink(&self, command_sink: NanoextCommandSink, shared : Shared) {
        // TODO: via take and get_or_insert
        let mut option = self.nanoext_command_sink.borrow_mut();
        let _maybe_old_sink = ::std::mem::replace(&mut *option, Some(command_sink));
        if let Some(next) = self.pending_nanoext_command.borrow_mut().take() {
            self.send_command_async(next, shared.clone());
        }
    }

    /// Asyncronously send a command to the nanoext.
    ///
    /// This will temporary comsume the sink and use the `pending_nanoext_command`
    /// to save the command while the sink is on its roundtrip.
    ///
    /// If another sink appeared when the original sink comes home (connection reset),
    /// then redo the command.
    pub fn send_command_async(&self, cmd : NanoExtCommand, shared : Shared) {
        match self.nanoext_command_sink.borrow_mut().take() {
            Some(sink) => {
                let send_and_return_home = sink.send(cmd).and_then(move |sink| {
                    // Put the `sink` to `shared` if the spot is still empty.
                    // I don't know if it can happen that the spot is already filled
                    // by a new connection. If the spot is filled, I would expect
                    // that the write on the old sink (that one we just got back) had failed.
                    // Then we would enter the `or_else` part.
                    if shared.nanoext_command_sink.borrow_mut().take().is_some() {
                        debug!("Successful send on old sink, but new sink present.");
                        // TODO: redo command! (should we use timestamps to be able to reject
                        // old commands?)
                    } else {
                        shared.nanoext_command_sink.borrow_mut().get_or_insert(sink);
                        // Trigger pending command
                        if let Some(next) = shared.pending_nanoext_command.borrow_mut().take() {
                            shared.send_command_async(next, shared.clone());
                        }
                    }
                    future::ok(())
                }).or_else(|error : Error| {
                    error!("Could not send command to Nanoext. ({})", error);
                    // We should trigger the next command, but we do not have a sink.
                    // The `put_command_sink` command will take care of a pending command.
                    future::err(())
                });
                self.spawn(send_and_return_home);
            },
            None => {
                // Put `cmd` into pending
                self.pending_nanoext_command.borrow_mut().take(); // clear old
                self.pending_nanoext_command.borrow_mut().get_or_insert(cmd);
            }
        }
    }
}


fn main() {
    match run() {
        Ok(()) => {}
        Err(e) => {
            error!("{:?}", e);
        }
    }
}

fn run() -> Result<(), Error> {
    init_logger();

    info!("START!");


    let (error_sink, error_stream) = mpsc::channel::<ErrorEvent>(1);
    let (shutdown_trigger, shutdown_shot) = oneshot::channel::<()>();
    // wrap trigger in closure that abstracts the consuming behaviour of send
    let mut shutdown_trigger = Some(shutdown_trigger);
    let shutdown_trigger = Box::new(move || {
        shutdown_trigger.take().map(|s_t| s_t.send(()).ok() /* no future involved here */ );
    });

    let shared = Rc::new(SharedInner {
        temperatures: Cell::new(TemperatureStats::default()),
        error_sink,
        pending_nanoext_command : RefCell::new(None),
        nanoext_command_sink : RefCell::new(None),
        reactor_handle: RefCell::new(None),
    });

    // Hyper will create the tokio core / reactor ...
    let server = web::make_web_server(&shared);
    // ... which we will reuse for the serial ports and everything else
    shared.put_handle(server.handle());

    let command_sink = nanoext::init_serial_port(shared.clone())?;
    shared.put_command_sink(command_sink, shared.clone());

    tlog20::init_serial_port(shared.clone())
        // ignore TLOG20 startup failure
        .map_err(|e| {
            error!("{}", e);
        })
        .ok();

    setup_ctrlc_handler(shared.clone());

    let stdin_stream = tokio_stdin::spawn_stdin_stream();
    handle_stdin_commands(stdin_stream, shared.clone());

    // Handle error events and restart serial port
    handle_errors(shared.clone(), error_stream, shutdown_trigger);

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


// The error handler will listen on the errorchannel and
// will restart the serial port if there was an error
// or it will gracefully quit the program when ctrl+c is pressed.
fn handle_errors(
    shared: Shared,
    error_stream: mpsc::Receiver<ErrorEvent>,
    mut shutdown_trigger: Box<FnMut() -> ()>,
) {
    let mut respawn_count: u32 = NANOEXT_RESPAWN_COUNT;

    // for moving into closure
    let shared1 = shared.clone();

    let error_handler = error_stream.for_each(move |error| {
        match error {
            // A Error in the NanoExt. Try to respawn max 5 times.
            ErrorEvent::NanoExtDecoderError => if respawn_count == 0 {
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
            ErrorEvent::CtrlC => {
                info!("Ctrl-c received.");
                shutdown_trigger();
            }
        }

        future::ok(())
    });

    shared.spawn(error_handler);
}


/// Handle Ctrl+c aka SIG_INT
fn setup_ctrlc_handler(shared: Shared) {
    let ctrl_c = tokio_signal::ctrl_c(&shared.handle()).flatten_stream();

    let shared1 = shared.clone();

    // Process each ctrl-c as it comes in
    let prog = ctrl_c.for_each(move |_| {
        shared1.handle_error_async(ErrorEvent::CtrlC);
        future::ok(())
    });

    shared.spawn(prog.map_err(|_| ()));
}


fn handle_stdin_commands(stdin_stream : tokio_stdin::StdInStream, shared: Shared) {

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