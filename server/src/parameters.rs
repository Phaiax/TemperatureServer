
use crossbeam_utils::atomic::AtomicCell;
use crate::sensors::{Sensor, Temperature};


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
