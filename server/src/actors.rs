
use async_std::prelude::*;
use async_std::fs;
use async_std::task::sleep;
use async_std::sync::Mutex;
use failure::{err_msg, Error, Fail, ResultExt};
use async_std::path::{Path, PathBuf};
use async_std::fs::{File};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use log::info;

pub struct Heater {
    /// Used to prevent concurrent access to the heater io
    mutex : Mutex<()>,
    gpio_pin : Vec<u8>,
    sysfs_gpio_export_path : PathBuf,
    sysfs_gpio_path : PathBuf,
    sysfs_gpio_value_path : PathBuf,
    sysfs_gpio_direction_path : PathBuf,
}

impl Heater {
    pub fn new(gpio_pin : u8) -> Heater {
        Heater {
            mutex : Mutex::new(()),
            gpio_pin: format!("{}", gpio_pin).into_bytes(),
            sysfs_gpio_export_path : PathBuf::from("/sys/class/gpio/export"),
            sysfs_gpio_path : PathBuf::from(format!("/sys/class/gpio/gpio{}", gpio_pin)),
            sysfs_gpio_value_path : PathBuf::from(format!("/sys/class/gpio/gpio{}/value", gpio_pin)),
            sysfs_gpio_direction_path : PathBuf::from(format!("/sys/class/gpio/gpio{}/direction", gpio_pin)),
        }
    }

    async fn assert_gpio_is_exported(&self) -> Result<(), Error> {
        if !self.sysfs_gpio_path.is_dir().await {
            fs::write(&self.sysfs_gpio_export_path, &self.gpio_pin).await?;
            sleep(Duration::from_millis(200)).await;
            fs::write(&self.sysfs_gpio_direction_path, "out").await?;
            sleep(Duration::from_millis(200)).await;
        }
        Ok(())
    }

    pub async fn turn_heater_on(&self) -> Result<(), Error> {
        self.assert_gpio_is_exported().await?;
        let _lock = self.mutex.lock().await; // prevent opening the sysfs_gpio_value_path twice at the same time
        if ! self.is_heater_on().await? {
            info!("Turn Plug on");
        }
        fs::write(&self.sysfs_gpio_value_path, "1").await?;
        Ok(())
    }

    pub async fn turn_heater_off(&self) -> Result<(), Error> {
        self.assert_gpio_is_exported().await?;
        let _lock = self.mutex.lock().await; // prevent opening the sysfs_gpio_value_path twice at the same time
        if self.is_heater_on().await? {
            info!("Turn Plug off");
        }
        fs::write(&self.sysfs_gpio_value_path, "0").await?;
        Ok(())
    }

    pub async fn is_heater_on(&self) -> Result<bool, Error> {
        self.assert_gpio_is_exported().await?;
        let data = fs::read(&self.sysfs_gpio_value_path).await?;
        Ok(data[0] == b'1')
    }

}


