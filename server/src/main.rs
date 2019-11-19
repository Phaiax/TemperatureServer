#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unreachable_code)]
#![recursion_limit = "128"]


mod sensors;
mod actors;
mod nanoext;
//mod web; // TODO
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
use std::time::Instant;
use std::path::PathBuf;
use std::thread;

use log::{LevelFilter, Record as LogRecord, log, info, warn, error, debug};
use env_logger::{Builder as LogBuilder, Target as LogTarget};
use dotenv::dotenv;

use serde_derive::{Serialize, Deserialize};

use async_std::prelude::*;
use async_std::task::spawn;
use async_std::sync::{channel, Sender, Receiver, Arc};
use async_std::stream::interval;
use futures::channel::oneshot;
use crossbeam_utils::atomic::AtomicCell;

use futures01::{future, Future};
use futures01::Stream;
use futures01::unsync::mpsc;
use futures01::Sink;

use tokio_core::reactor::{Handle};
use tokio_inotify::AsyncINotify;

use failure::{err_msg, Error, Fail, ResultExt};

use chrono::prelude::*;

use crate::nanoext::{NanoExtCommand, NanoextCommandSink};
use crate::temp::TemperatureStats;

use crate::shared::{setup_shared, Shared, SharedInner};

use file_db::{FileDb, Timestamped};

use crate::utils::FutureExt;

pub const NANOEXT_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0";
pub const TLOG20_SERIAL_DEVICE: &'static str =
    "/dev/serial/by-id/usb-FTDI_US232R_FT0ILAKP-if00-port0";
pub const NANOEXT_RESPAWN_COUNT: u32 = 5;

pub const STDIN_BUFFER_SIZE: usize = 1000;

// pub const TEMPERATURE_STORAGE_INTERVAL_SECONDS : usize

pub const SENSOR_FILE_PATH: &'static str =
    "/home/pi/sensors";
pub const HEATER_GPIO: u8 = 17;


#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HeaterControlStrategy {
    /// Use the trigger values defined in `SharedInner::parameters`
    Auto,
    ForceOn { until: Instant },
    ForceOff { until: Instant },
}

pub enum Event {
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
    async fn new_from_current(shared: &Shared) -> TSDataLogEntry {
        let temps = shared.temperatures.load();
        let plug_state = shared.heater.is_heater_on().await.unwrap_or(false);
        //let reference = shared1.
        let mut new = DataLogEntry {
            mean: [0; 6],
            celsius: crate::temp::raw2celsius100(&temps.mean),
            plug_state,
            reference_celsius: shared
                .reference_temperature
                .load()
                .map(|temp_in_degc| (temp_in_degc * 100.) as i16),
        };
        new.mean
            .iter_mut()
            .zip(temps.mean.iter())
            .for_each(|(n, m)| *n = *m as u16);
        Timestamped::now(new)
    }
}

#[async_std::main]
async fn main() -> Result<(), Error> {
    // Init logging
    init_logger();

    // Sync environment from .env
    dotenv().ok();

    // Oneshot to exit event loop
    #[allow(unused_variables)]
    let (shutdown_trigger, shutdown_shot) = setup_shutdown_oneshot();

    // Channel to the high-level event loop (see `handle_events()`)
    let (event_sink, event_stream) = channel::<Event>(1);

    // Open Database and create thread for asyncronity
    let db = MyFileDb::new_from_env("LOG_FOLDER", 2, "v2").await?;

    // Setup shared data (Handle and CommandSink still missing)
    let shared = setup_shared(event_sink, db, HEATER_GPIO);

    // Hyper will create the tokio core / reactor ...
    // let server = web::make_web_server(&shared)?; TODO

    // ... which we will reuse for the serial ports and everything else
    //shared.put_handle(server.handle()); TODO

    // Now connect to the Arduino (called the NanoExt)

    // Now connect to the TLOG20, but do not fail if not present
    tlog20::init_serial_port(shared.clone())
        .map_err(|err| error!("{}", err))
        .ok();

    // Handle on-the-fly attachment of external hardware
    //setup_serial_watch_and_reinit(shared.clone())?;

    // Handle SIG_INT and SIG_TERM
    setup_ctrlc_and_sigint_forwarding(shared.clone());

    // Handle stdin. Command interpreting occours here.
    spawn(stdin_handler_loop(shared.clone()));

    // Handle events, including error events and do restart serial port.
    // Events can be sent via `shared.handle_event_async()`
    setup_main_event_handler(shared.clone(), event_stream, shutdown_trigger);

    // save regulary
    setup_db_save_interval(Duration::from_secs(60*10), shared.clone());

    // A manually commanded ForceOn/ForceOff will reset to Auto after a few minutes
    setup_plug_command_timeout_handler(shared.clone());

    // Init plug off
    shared.heater.turn_heater_off().await?;

    info!("START EVENT LOOP!");

    // Start Webserver and block until shutdown is triggered
    unimplemented!();
    //server.run_until(shutdown_shot.map_err(|_| ())).unwrap();

    // save on shutdown
    shared.db().save_all().await.unwrap();

    info!("SHUTDOWN! {}", Arc::strong_count(&shared));

    Ok(())
}

pub fn init_logger() {
    use std::io::Write;
    let mut builder = LogBuilder::from_default_env();
    builder.target(LogTarget::Stdout);
    builder.format(|buf, record| {
        let local: DateTime<Local> = Local::now();
        writeln!(buf,
            "{} - {} - {}",
            local.to_rfc2822(),
            record.level(),
            record.args()
        )
    });
    builder.filter(None, LevelFilter::Info);
    builder.init();
}

type ShutdownTrigger = Box<dyn FnMut() -> () + Send>;
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
fn setup_main_event_handler(
    shared: Shared,
    mut event_stream: Receiver<Event>,
    mut shutdown_trigger: ShutdownTrigger,
) {
    spawn(async move {
        while let Some(event) = event_stream.next().await {
            match event {
                Event::NewTemperatures => {
                    heater_ctrl(&shared).await;

                    shared
                        .db()
                        .insert_or_update_async(DataLogEntry::new_from_current(&shared).await)
                        .print_and_forget_error().await;
                }
                Event::CtrlC => {
                    info!("Ctrl-c received.");
                    shutdown_trigger();
                }
                Event::SigTerm => {
                    info!("SIGTERM received.");
                    shutdown_trigger();
                }
            }

        }
    });
}

async fn heater_ctrl(shared: &Shared) {
    match shared.plug_command.load() {
        HeaterControlStrategy::Auto => {
            let current : f64 = shared.temperatures.load().mean[usize::from(shared.parameters.use_sensor)]; // Lower
            if current >= shared.parameters.plug_trigger_off.load().0 {
                shared.heater.turn_heater_off().print_and_forget_error().await;
            } else if current <= shared.parameters.plug_trigger_on.load().0 {
                shared.heater.turn_heater_on().print_and_forget_error().await;
            } else {
                // KeepAsIs
            }
        }
        HeaterControlStrategy::ForceOn { .. } => {
            shared.heater.turn_heater_on().print_and_forget_error().await;
        }
        HeaterControlStrategy::ForceOff { .. } => {
            shared.heater.turn_heater_off().print_and_forget_error().await;
        }
    }
}


pub fn setup_db_save_interval(every: Duration, shared: Shared) {
    spawn(async move {
        let mut interval = interval(every);
        while let Some(_) = interval.next().await {
            shared.db().save_all().print_and_forget_error_with_context("Could not save database").await;
        }
    });
}

/// Handle Ctrl+c aka SIG_INT
fn setup_ctrlc_and_sigint_forwarding(shared: Shared) {
    use signal_hook::iterator::Signals;

    let signals = Signals::new(&[
        signal_hook::SIGTERM,
        signal_hook::SIGINT,
    ]).unwrap();

    thread::spawn(move || {
        for signal in signals.forever() {
            match signal {
                signal_hook::SIGTERM => {
                    shared.handle_event_async(Event::SigTerm);
                },
                signal_hook::SIGINT => {
                    shared.handle_event_async(Event::CtrlC);
                },
                _ => unreachable!(),
            }
        }
    });

}


async fn stdin_handler_loop(shared: Shared) -> Result<(), Error> {

    let stdin_stream = async_std::io::stdin();
    let mut line = String::new();
    #[allow(irrefutable_let_patterns)]
    while let _read_bytes = stdin_stream.read_line(&mut line).await? {
        match line.as_str() {
            "rc" => {
                info!(
                    "Strong count of shared: {}",
                    async_std::sync::Arc::strong_count(&shared)
                );
            }
            "1" => {
                shared.plug_command.store(HeaterControlStrategy::ForceOn{ until: Instant::now() + Duration::from_secs(3600 * 12) });
                heater_ctrl(&shared).await;
            }
            "0" => {
                shared.plug_command.store(HeaterControlStrategy::ForceOff{ until: Instant::now() + Duration::from_secs(3600 * 12) });
                heater_ctrl(&shared).await;
            }
            "a" => {
                shared.plug_command.store(HeaterControlStrategy::Auto);
                heater_ctrl(&shared).await;
            }
            _ => {}
        }

        line.clear();
    }
    Ok(())
}


fn setup_plug_command_timeout_handler(shared: Shared) {
    spawn(async move {
        let mut interval = interval(Duration::from_secs(1));
        while let Some(_) = interval.next().await {

            let curr = shared.plug_command.load();
            let new = match curr {
                HeaterControlStrategy::Auto => HeaterControlStrategy::Auto,
                HeaterControlStrategy::ForceOn { until } => {
                    if until <= Instant::now() {
                        HeaterControlStrategy::Auto
                    } else {
                        HeaterControlStrategy::ForceOn { until }
                    }
                },
                HeaterControlStrategy::ForceOff { until } => {
                    if until <= Instant::now() {
                        HeaterControlStrategy::Auto
                    } else {
                        HeaterControlStrategy::ForceOff { until }
                    }
                }
            };
            if new != curr {
                shared.plug_command.store(new);
            }
        }
    });
}

impl HeaterControlStrategy {
    pub fn is_auto(&self) -> bool {
        match self {
            &HeaterControlStrategy::Auto => true,
            _ => false,
        }
    }
}