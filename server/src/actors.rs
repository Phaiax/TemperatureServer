
use std::mem;
use std::rc::Rc;
use std::time::Duration;
use async_std::task::sleep;

use async_std::sync::Mutex;

use failure::{Error, ResultExt};
use log::info;

use async_std_gpiod::{Chip, Options, Lines, Output};

pub struct Heater {
    /// Used to prevent concurrent access to the heater io
    mutex : Mutex<()>,
    outputs: Lines<Output>,
}

impl Heater {
    pub async fn new(gpio_pin : u8) -> Result<Heater, Error> {
        let chip = Rc::new(Chip::new("gpiochip0").await?);
        let opts = Options::output([gpio_pin as u32]) // lines offsets
                .values([false]) // initial values
                .consumer("temperatures");
        let outputs = chip.request_lines(opts).await?;
        mem::forget(chip);
        Ok(Heater {
            mutex : Mutex::new(()),
            outputs,
        })
    }

    pub async fn turn_heater_on(&self) -> Result<(), Error> {
        let _lock = self.mutex.lock().await; // prevent opening the sysfs_gpio_value_path twice at the same time
        if ! self.is_heater_on().await? {
            info!("Turn Plug on");
        }
        self.outputs.set_values([true]).await?;
        Ok(())
    }

    pub async fn turn_heater_off(&self) -> Result<(), Error> {
        let _lock = self.mutex.lock().await; // prevent opening the sysfs_gpio_value_path twice at the same time
        if self.is_heater_on().await? {
            info!("Turn Plug off");
        }
        self.outputs.set_values([false]).await?;
        Ok(())
    }

    pub async fn is_heater_on(&self) -> Result<bool, Error> {
        Ok(self.outputs.get_values([false]).await?[0])
    }

}


