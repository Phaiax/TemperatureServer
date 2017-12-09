
use std::cell::Cell;
use std::rc::Rc;

use temp::TemperatureStats;

pub type SharedDataRc = Rc<SharedData>;

pub struct SharedData {
    pub temperatures: Cell<TemperatureStats>,
}

impl SharedData {
    pub fn new_rc() -> SharedDataRc {
        Rc::new(SharedData {
            temperatures: Cell::new(TemperatureStats::default()),
        })
    }
}
