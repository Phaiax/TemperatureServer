#![allow(unused_imports)]
#![allow(dead_code)]
#![recursion_limit = "128"]


mod nanoext;
mod web;
mod temp;
mod tlog20;
mod shared;
mod parameters;
mod utils;

use std::sync::RwLock;
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::env;
use std::time::Duration;
use std::path::PathBuf;

use log::{LogLevelFilter, LogRecord, log, info, warn, error, debug};
use env_logger::{LogBuilder, LogTarget};
use dotenv::dotenv;

use serde_derive::{Serialize, Deserialize};

use futures::{future, Future};
use futures::Stream;
use futures::unsync::mpsc;
use futures::unsync::oneshot;
use futures::Sink;

use tokio_core::reactor::{Handle, Interval};
use tokio_signal::unix::Signal;
use tokio_inotify::AsyncINotify;

use failure::{err_msg, Error, Fail, ResultExt};

use chrono::prelude::*;

use crate::nanoext::{NanoExtCommand, NanoextCommandSink};
use crate::temp::TemperatureStats;

use crate::shared::{setup_shared, Shared, SharedInner, PlugCommand};

use file_db::{FileDb, Timestamped};

use crate::parameters::PlugAction;

use crate::utils::FutureExt;

pub const NANOEXT_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";
pub const TLOG20_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-FTDI_US232R_FT0ILAKP-if00-port0";
pub const NANOEXT_RESPAWN_COUNT: u32 = 5;

pub const STDIN_BUFFER_SIZE: usize = 1000;

// pub const TEMPERATURE_STORAGE_INTERVAL_SECONDS : usize


pub enum Event {
    NanoExtDecoderError,
    CtrlC,
    SigTerm,
    NewTemperatures,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DataLogEntry {
    pub mean: [u16; 6],
    pub celsius: [i16; 6],
    pub plug_state: bool,
    pub reference_celsius: Option<i16>,
}

pub type TSDataLogEntry = Timestamped<DataLogEntry>;
pub type MyFileDb = FileDb<TSDataLogEntry>;

impl DataLogEntry {
    fn new_from_current(shared: &Shared) -> TSDataLogEntry {
        let temps = shared.temperatures.get();
        let plug_state = shared.plug_state.get();
        //let reference = shared1.
        let mut new = DataLogEntry {
            mean: [0; 6],
            celsius: crate::temp::raw2celsius100(&temps.mean),
            plug_state,
            reference_celsius: shared
                .reference_temperature
                .get()
                .map(|temp_in_degc| (temp_in_degc * 100.) as i16),
        };
        new.mean
            .iter_mut()
            .zip(temps.mean.iter())
            .for_each(|(n, m)| *n = *m as u16);
        Timestamped::now(new)
    }
}

fn main() {
    // Init logging
    init_logger();

    // Sync environment from .env
    dotenv().ok();

    match run() {
        Ok(()) => {}
        Err(err) => {
            utils::print_error_and_causes(err);
        }
    }
}

fn run() -> Result<(), Error> {
    // Oneshot to exit event loop
    let (shutdown_trigger, shutdown_shot) = setup_shutdown_oneshot();

    // Channel to the high-level event loop (see `handle_events()`)
    let (event_sink, event_stream) = mpsc::channel::<Event>(1);

    // Open Database and create thread for asyncronity
    let db = MyFileDb::new_from_env("LOG_FOLDER", 2, "v2")?;

    // Setup shared data (Handle and CommandSink still missing)
    let shared = setup_shared(event_sink, db);

    // Hyper will create the tokio core / reactor ...
    let server = web::make_web_server(&shared)?;

    // ... which we will reuse for the serial ports and everything else
    shared.put_handle(server.handle());

    // Now connect to the Arduino (called the NanoExt)
    match nanoext::init_serial_port(shared.clone()) {
        Ok(command_sink) => shared.put_command_sink(command_sink, shared.clone()),
        Err(err) => /* return Err(err) */ error!("{}", err)
    }


    // Now connect to the TLOG20, but do not fail if not present
    tlog20::init_serial_port(shared.clone())
        .map_err(|err| error!("{}", err))
        .ok();

    // Handle on-the-fly attachment of external hardware
    setup_serial_watch_and_reinit(shared.clone())?;

    // Handle SIG_INT
    setup_ctrlc_forwarding(shared.clone());

    // Handle stdin. Command interpreting occours here.
    setup_stdin_handling(shared.clone());

    // Handle SIGTERM for service
    setup_sigterm_forwarding(shared.clone());

    // Handle events, including error events and do restart serial port.
    // Events can be sent via `shared.handle_event_async()`
    handle_events(shared.clone(), event_stream, shutdown_trigger);

    // save regulary
    setup_db_save_interval(Duration::from_secs(60*10), shared.clone());

    // Init plug off
    shared.send_command_async(NanoExtCommand::PowerOff, shared.clone());
    shared.plug_state.set(false);

    info!("START EVENT LOOP!");

    // Start Webserver and block until shutdown is triggered
    server.run_until(shutdown_shot.map_err(|_| ())).unwrap();

    // save on shutdown
    shared.db().save_all().wait().unwrap();

    info!("SHUTDOWN! {}", Rc::strong_count(&shared));

    Ok(())
}

pub fn init_logger() {
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

type ShutdownTrigger = Box<dyn FnMut() -> ()>;
type ShutdownShot = oneshot::Receiver<()>;

fn setup_shutdown_oneshot() -> (ShutdownTrigger, ShutdownShot) {
    let (shutdown_trigger, shutdown_shot) = oneshot::channel::<()>();
    // wrap trigger in closure that abstracts the consuming behaviour of send
    let mut shutdown_trigger = Some(shutdown_trigger);
    let shutdown_trigger = Box::new(move || {
        shutdown_trigger
            .take()
            .map(|s_t| s_t.send(()).ok() /* no future involved here */);
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
    event_stream: mpsc::Receiver<Event>,
    mut shutdown_trigger: ShutdownTrigger,
) {
    let mut respawn_count: u32 = NANOEXT_RESPAWN_COUNT;

    // for moving into closure
    let shared1 = shared.clone();

    let event_handler = event_stream.for_each(move |error| {
        match error {
            Event::NewTemperatures => {
                let filedb_update = shared1
                    .db()
                    .insert_or_update_async(DataLogEntry::new_from_current(&shared1));
                shared1.spawn(filedb_update.print_and_forget_error());

                PlugCommand::check_elapsed(&shared1.plug_command);
                match shared1.plug_command.get() {
                    PlugCommand::Auto => {
                        let action = shared1.parameters.plug_action(&shared1.temperatures.get(),
                                                                    shared1.reference_temperature.get());
                        match action {
                            PlugAction::TurnOn => if !shared1.plug_state.get() {
                                info!("Turn Plug on");
                                shared1.send_command_async(NanoExtCommand::PowerOn, shared1.clone());
                                shared1.plug_state.set(true);
                            },
                            PlugAction::TurnOff => if shared1.plug_state.get() {
                                info!("Turn Plug off");
                                shared1.send_command_async(NanoExtCommand::PowerOff, shared1.clone());
                                shared1.plug_state.set(false);
                            },
                            PlugAction::KeepAsIs => {}
                        }
                    },
                    PlugCommand::ForceOn { .. } => {
                        if !shared1.plug_state.get() {
                            info!("Turn Plug on (manual)");
                            shared1.send_command_async(NanoExtCommand::PowerOn, shared1.clone());
                            shared1.plug_state.set(true);
                        }
                    },
                    PlugCommand::ForceOff { .. } => {
                        if shared1.plug_state.get() {
                            info!("Turn Plug off (manual)");
                            shared1.send_command_async(NanoExtCommand::PowerOff, shared1.clone());
                            shared1.plug_state.set(false);
                        }
                    },
                }
            }
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
            Event::SigTerm => {
                info!("SIGTERM received.");
                shutdown_trigger();
            }
        }

        future::ok(())
    });

    shared.spawn(event_handler);
}

pub fn setup_db_save_interval(interval: Duration, shared: Shared) {
    let shared1 = shared.clone();
    let prog = Interval::new(interval, &shared.handle())
        .unwrap()
        .for_each(move |_| {
            let prog = shared1.db().save_all();
            shared1.spawn(prog.print_and_forget_error_with_context("Could not save database"));
            future::ok(())
        });
    shared.spawn(prog.print_and_forget_error());
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

    shared.spawn(prog.print_and_forget_error_with_context("Ctrl+C forwarding error."));
}

fn setup_sigterm_forwarding(shared: Shared) {
    let shared1 = shared.clone();

    let prog = Signal::new(::libc::SIGTERM, &shared.handle())
        .map(|x| Box::new(x.map(|_| ())))
        .flatten_stream()
        .for_each(move |_| {
            shared1.handle_event_async(Event::SigTerm);
            future::ok(())
        });

    shared.spawn(prog.print_and_forget_error_with_context("SIGTERM forwarding error."));
}


fn setup_stdin_handling(shared: Shared) {
    let stdin_stream = tokio_stdin::spawn_stdin_stream();

    let shared1 = shared.clone();

    let prog = stdin_stream
        .for_each(move |line| {
            match line.as_str() {
                "rc" => {
                    info!(
                        "Strong count of shared: {}",
                        ::std::rc::Rc::strong_count(&shared1)
                    );
                }
                "1" => shared1.send_command_async(NanoExtCommand::PowerOn, shared1.clone()),
                "0" => shared1.send_command_async(NanoExtCommand::PowerOff, shared1.clone()),
                _ => {}
            }
            future::ok(())
        })
        .map_err(|_| {
            // Error has type ()
            debug!("StdIn canceled.");
            ()
        });

    shared.spawn(prog);
}

fn setup_serial_watch_and_reinit(shared: Shared) -> Result<(), Error> {
    let path_notify = AsyncINotify::init(&shared.handle())?;


    let device_path = PathBuf::from(NANOEXT_SERIAL_DEVICE);
    let dev_serial = device_path.parent().ok_or(err_msg(
        "Can't make parent directory of serial device paths",
    ))?;

    const IN_CREATE: u32 = 256;
    path_notify
        .add_watch(dev_serial, IN_CREATE)
        .context("Can not watch serial devices' parent directory.")?;

    let new_serial_watch = path_notify.for_each(move |_event| {
        println!("{:?}", &_event.name.to_string_lossy());
        future::ok(())
    });

    shared.spawn(new_serial_watch.print_and_forget_error());


    Ok(())
}

// from crate `tokio-stdin`
// But buffer lines instead of sending each byte by itself
mod tokio_stdin {

    use futures::stream::iter_result;
    use futures::{Future, Sink, Stream};
    use futures::sync::mpsc::{channel, Receiver, SendError};
    use std::io::{self, BufRead};
    use std::thread;

    pub type Line = String;
    pub type StdInStream = Receiver<Line>;

    #[derive(Debug)]
    pub enum StdinError {
        Stdin(::std::io::Error),
        Channel(SendError<Line>),
    }

    #[allow(unused_must_use)]
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
