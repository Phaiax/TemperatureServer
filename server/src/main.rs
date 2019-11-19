#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unreachable_code)]
#![recursion_limit = "128"]


mod sensors;
mod actors;
//mod web; // TODO
mod tlog20;
mod utils;

use std::sync::RwLock;
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::env;
use std::time::Duration;
use std::time::Instant;
use std::path::PathBuf;
use std::thread;
use std::sync::atomic::AtomicBool;

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

use crate::sensors::{Temperature, Temperatures, Sensor};
use crate::actors::Heater;

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

    // Setup shared data
    let shared = Arc::new(SharedInner {
        temperatures: AtomicCell::new(Temperatures::default()),
        event_sink,
        heater : Heater::new(HEATER_GPIO),
        control_strategy : AtomicCell::new(HeaterControlMode::Auto),
        reference_temperature : AtomicCell::new(None),
        tlog20_connected: AtomicBool::new(false),
        db,
        parameters : Parameters::default(),
    });

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

    // Handle SIG_INT and SIG_TERM and forward as events to main loop
    setup_ctrlc_and_sigint_forwarding(shared.clone()); // starts a thread

    // Handle stdin. Command interpreting occours here.
    spawn(stdin_handler_loop(shared.clone()));

    // Events can be sent via `shared.handle_event_async()`
    spawn(main_event_handler_loop(shared.clone(), event_stream, shutdown_trigger));

    // save regulary
    spawn(save_database_loop(Duration::from_secs(60*10), shared.clone()));

    // A manually commanded ForceOn/ForceOff will reset to Auto after a few minutes
    spawn(control_strategy_timeout_loop(shared.clone()));

    // Init plug off
    shared.heater.turn_heater_off().await?;

    info!("START EVENT LOOP!");

    // Start Webserver and block until shutdown is triggered
    unimplemented!();
    //server.run_until(shutdown_shot.map_err(|_| ())).unwrap();

    // save on shutdown
    shared.db.save_all().await.unwrap();

    info!("SHUTDOWN! {}", Arc::strong_count(&shared));

    Ok(())
}

// ============================= Misc ===============================


/// Mode of operation for the heater
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HeaterControlMode {
    /// Use the hysteresis defined in `SharedInner::parameters`
    Auto,
    ForceOn { until: Instant },
    ForceOff { until: Instant },
}

/// Event variants that can be sent to the main event handler loop.
/// These events can be sent via `shared.handle_event_async()`, even from sync code.
pub enum Event {
    CtrlC,
    SigTerm,
    NewTemperatures,
}

pub struct Parameters {
    pub plug_trigger_on: AtomicCell<Temperature>,
    pub plug_trigger_off: AtomicCell<Temperature>,
    pub use_sensor: Sensor,
}


impl Default for Parameters {
    fn default() -> Parameters {
        Parameters {
            plug_trigger_on: AtomicCell::new(Temperature::from_celsius(0.5)),
            plug_trigger_off: AtomicCell::new(Temperature::from_celsius(0.7)),
            //plug_trigger_on: AtomicCell::new(Celsius(22.5)),
            //plug_trigger_off: AtomicCell::new(Celsius(23.0)),
            use_sensor: Sensor::Fourth,
        }
    }
}

// ============================= Shared context ===============================

/// The shared context that can be used to access the main loop, get sensor data,
/// control the heater and access the database.
pub type Shared = Arc<SharedInner>;

pub struct SharedInner {
    /// Most recent received temperatures
    pub temperatures: AtomicCell<Temperatures>,
    /// Access to actor
    pub heater : Heater,
    /// Current control strategy
    pub control_strategy : AtomicCell<HeaterControlMode>,
    /// Reference temperature available
    pub tlog20_connected : AtomicBool,
    /// TLOG 20 reference temperature (typically not available)
    pub reference_temperature : AtomicCell<Option<f64>>,
    /// Queue to main event loop
    pub event_sink: Sender<Event>,
    /// Database
    pub db : MyFileDb,
    /// Control parameters (Seonsor and hysteresis)
    pub parameters : Parameters,
}

impl SharedInner {
    pub fn handle_event_async(&self, e: Event) {
        let event_sink = self.event_sink.clone();
        spawn(async move { event_sink.send(e).await; });
    }
}


// ============================= Database ===============================

/// The FileDB that is used
pub type MyFileDb = FileDb<TSDataLogEntry>;

/// The entry type for our specialization of FileDb.
/// Wrapping by `Timestamped` allows chunking by date
pub type TSDataLogEntry = Timestamped<DataLogEntry>;

/// The inner entry type for our specialization of FileDb.
/// (Will be wrapped by file_db::Timestamped)
/// Keep the layout constant!
#[derive(Clone, Serialize, Deserialize)]
pub struct DataLogEntry {
    /// This field contained the filtered raw values from the temperature ADCs.
    /// Since there is no analog measurement anymore, this field is now unused
    /// But we must not change the layout of this struct because this is the database
    /// entry type.
    pub _mean: [u16; 6],
    /// The temperature in deg Celsius * 100
    /// This unit plays well with `sensors::Temperature::from_raw/to_raw`.
    pub celsius: [i16; 6],
    /// The heater state
    pub plug_state: bool,
    /// The temperature from the TLOG 20 device, if plugged in
    pub reference_celsius: Option<i16>,
}


impl DataLogEntry {
    async fn new_from_current(shared: &Shared) -> TSDataLogEntry {
        Timestamped::now(DataLogEntry {
            _mean: [0; 6],
            celsius: shared.temperatures.load().as_raw_with_default(Temperature::from_raw(0)),
            plug_state: shared.heater.is_heater_on().await.unwrap_or(false),
            reference_celsius: shared
                .reference_temperature
                .load()
                .map(|temp_in_degc| (temp_in_degc * 100.) as i16),
        })
    }
}

// ============================= Initialization ===============================

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

// ============================= Graceful shutdown helpers =====================


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


// ============================= async loops / threads / control ================


/// Events can be sent via `shared.handle_event_async()`
///
/// The error handler will listen on the errorchannel and
/// will restart the serial port if there was an error
/// or it will gracefully quit the program when ctrl+c is pressed.
async fn main_event_handler_loop(
    shared: Shared,
    mut event_stream: Receiver<Event>,
    mut shutdown_trigger: ShutdownTrigger,
) {
    while let Some(event) = event_stream.next().await {
        match event {
            Event::NewTemperatures => {
                heater_ctrl(&shared).await;

                shared
                    .db
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
}

async fn heater_ctrl(shared: &Shared) {
    match shared.control_strategy.load() {
        HeaterControlMode::Auto => {
            match shared.temperatures.load().get(shared.parameters.use_sensor) {
                Some(current) => {
                    if current >= shared.parameters.plug_trigger_off.load() {
                        shared.heater.turn_heater_off().print_and_forget_error().await;
                    } else if current <= shared.parameters.plug_trigger_on.load() {
                        shared.heater.turn_heater_on().print_and_forget_error().await;
                    } else {
                        // KeepAsIs
                    }
                },
                None => {
                    // Temperature not available -> Turn off
                    shared.heater.turn_heater_off().print_and_forget_error().await;
                }
            }
        }
        HeaterControlMode::ForceOn { .. } => {
            shared.heater.turn_heater_on().print_and_forget_error().await;
        }
        HeaterControlMode::ForceOff { .. } => {
            shared.heater.turn_heater_off().print_and_forget_error().await;
        }
    }
}


async fn save_database_loop(every: Duration, shared: Shared) {
    let mut interval = interval(every);
    while let Some(_) = interval.next().await {
        shared.db.save_all().print_and_forget_error_with_context("Could not save database").await;
    }
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
                shared.control_strategy.store(HeaterControlMode::ForceOn{ until: Instant::now() + Duration::from_secs(3600 * 12) });
                heater_ctrl(&shared).await;
            }
            "0" => {
                shared.control_strategy.store(HeaterControlMode::ForceOff{ until: Instant::now() + Duration::from_secs(3600 * 12) });
                heater_ctrl(&shared).await;
            }
            "a" => {
                shared.control_strategy.store(HeaterControlMode::Auto);
                heater_ctrl(&shared).await;
            }
            _ => {}
        }

        line.clear();
    }
    Ok(())
}


async fn control_strategy_timeout_loop(shared: Shared) {
    let mut interval = interval(Duration::from_secs(1));
    while let Some(_) = interval.next().await {

        let curr = shared.control_strategy.load();
        let new = match curr {
            HeaterControlMode::Auto => HeaterControlMode::Auto,
            HeaterControlMode::ForceOn { until } => {
                if until <= Instant::now() {
                    HeaterControlMode::Auto
                } else {
                    HeaterControlMode::ForceOn { until }
                }
            },
            HeaterControlMode::ForceOff { until } => {
                if until <= Instant::now() {
                    HeaterControlMode::Auto
                } else {
                    HeaterControlMode::ForceOff { until }
                }
            }
        };
        if new != curr {
            shared.control_strategy.store(new);
        }
    }
}
