
use async_std::prelude::*;
use async_std::sync::{Arc, Mutex};
use async_std::task::spawn;
use async_std::sync::{Sender, Receiver};
use crossbeam_utils::atomic::AtomicCell;


use log::{log, error}; // macro
use crate::nanoext::{NanoExtCommand, NanoextCommandSink};
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use crate::temp::TemperatureStats;
use tokio_core::reactor::Handle;
use futures01::{future, Future, Sink};
use futures01::unsync::mpsc;
use failure::Error;
use crate::Event;
use crate::parameters::Parameters;
use crate::MyFileDb;
use crate::actors::Heater;

use super::HeaterControlStrategy;


// Todo: make newtype struct and create accessor methods for pub fields. Then remove shared argument
// in the member functions of SharedInner below. (do impl Shared instead)
pub type Shared = Arc<SharedInner>;

pub struct SharedInner {
    pub temperatures: AtomicCell<TemperatureStats>,
    pub heater : Heater,
    pub plug_command : AtomicCell<HeaterControlStrategy>,
    pub reference_temperature : AtomicCell<Option<f64>>,
    event_sink: Sender<Event>,
    pub tlog20_connected : AtomicBool,
    db : MyFileDb,
    pub parameters : Parameters,
}

pub fn setup_shared(event_sink : Sender<Event>, db : MyFileDb, gpio: u8) -> Shared {

    Arc::new(SharedInner {
        temperatures: AtomicCell::new(TemperatureStats::default()),
        event_sink,
        heater : Heater::new(gpio),
        plug_command : AtomicCell::new(HeaterControlStrategy::Auto),
        reference_temperature : AtomicCell::new(None),
        tlog20_connected: AtomicBool::new(false),
        db,
        parameters : Parameters::default(),
    })
}

impl SharedInner {
    pub fn handle_event_async(&self, e: Event) {
        let event_sink = self.event_sink.clone();
        spawn(async move { event_sink.send(e).await; });
    }


    pub fn db(&self) -> &MyFileDb {
        &self.db
    }

}
