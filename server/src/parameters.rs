
use std::cell::Cell;
use temp::TemperatureStats;

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
pub struct Celsius(f64);

pub struct Parameters {
    plug_trigger_on: Cell<Celsius>,
    plug_trigger_off: Cell<Celsius>,
    use_sensor: Sensor,
}

pub enum PlugAction {
    TurnOn,
    TurnOff,
    KeepAsIs,
}

impl Parameters {
    pub fn plug_action(&self, current: &TemperatureStats) -> PlugAction {
        let sensor_id: usize = self.use_sensor.into();
        let current : f64 = ::temp::raw2celsius(&current.mean)[sensor_id];
        if current >= self.plug_trigger_off.get().0 {
            PlugAction::TurnOff
        } else if current <= self.plug_trigger_on.get().0 {
            PlugAction::TurnOn
        } else {
            PlugAction::KeepAsIs
        }
    }
}

impl Default for Parameters {
    fn default() -> Parameters {
        Parameters {
            // plug_trigger_on: Cell::new(Celsius(-0.2)),
            // plug_trigger_off: Cell::new(Celsius(0.2)),
            plug_trigger_on: Cell::new(Celsius(22.5)),
            plug_trigger_off: Cell::new(Celsius(23.0)),
            use_sensor: Sensor::Sec,
        }
    }
}
