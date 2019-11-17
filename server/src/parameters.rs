
use std::cell::Cell;
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
    pub fn plug_action(&self, current: &TemperatureStats, reference : Option<f64>) -> PlugAction {
        let _sensor_id: usize = self.use_sensor.into();
        let mut all : [f64;6] = crate::temp::raw2celsius(&current.mean);
        // skip outdoor. Hack: for now replace outdoor with 10 or the reference
        all[5] = 10.;
        match reference {
            Some(s) => { all[5] = s; },
            _ => {}
        }
        let mut current : f64 = all.iter().fold(100., |min, i| i.min(min) );
        if current == 100. {
            current = 0.;
        }
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
            plug_trigger_on: Cell::new(Celsius(0.5)),
            plug_trigger_off: Cell::new(Celsius(0.7)),
            //plug_trigger_on: Cell::new(Celsius(22.5)),
            //plug_trigger_off: Cell::new(Celsius(23.0)),
            use_sensor: Sensor::Fourth,
        }
    }
}
