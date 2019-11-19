
use crossbeam_utils::atomic::AtomicCell;
use crate::temp::TemperatureStats;

#[derive(Clone, Copy, Debug)]
pub enum Sensor {
    // in the order of the serial data
    Top,
    Sec,
    Third,
    Fourth,
    Bottom,
    Outdoor,
}

impl From<Sensor> for usize {
    fn from(sensor : Sensor) -> usize {
        match sensor {
            Sensor::Top => 0,
            Sensor::Sec => 1,
            Sensor::Third => 2,
            Sensor::Fourth => 3,
            Sensor::Bottom => 4,
            Sensor::Outdoor => 5,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Celsius(pub f64);

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
