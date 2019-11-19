
use crossbeam_utils::atomic::AtomicCell;
use crate::temp::TemperatureStats;
use crate::sensors::{Sensor, Celsius};


pub struct Parameters {
    pub plug_trigger_on: AtomicCell<Celsius>,
    pub plug_trigger_off: AtomicCell<Celsius>,
    pub use_sensor: Sensor,
}


impl Default for Parameters {
    fn default() -> Parameters {
        Parameters {
            plug_trigger_on: AtomicCell::new(Celsius(0.5)),
            plug_trigger_off: AtomicCell::new(Celsius(0.7)),
            //plug_trigger_on: AtomicCell::new(Celsius(22.5)),
            //plug_trigger_off: AtomicCell::new(Celsius(23.0)),
            use_sensor: Sensor::Fourth,
        }
    }
}
