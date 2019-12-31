use std::boxed::Box;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use std::fmt;

use async_std::fs::File;
use async_std::path::PathBuf;
use async_std::prelude::*;
use async_std::stream::{interval, Interval};
use futures::future::join_all;
use futures::future::FutureExt;

use failure::{bail, Error, ResultExt};
use log::info;

use serde_derive::{Serialize, Deserialize};

use crate::SENSOR_FILE_PATH;

/// Temperature.
/// The internal temperature is storea as (deg Celsius times 100)
#[derive(Copy, Clone, Debug, Serialize, Deserialize, Default, Ord, PartialOrd, Eq, PartialEq)]
pub struct Temperature(pub i16);

impl Temperature {
    pub fn from_celsius(celsius : f32) -> Temperature { Temperature((celsius * 100.0) as i16) }
    pub fn from_raw(raw : i16) -> Temperature { Temperature(raw) }
    pub fn as_celsius(&self) -> f32 { (self.0 as f32) / 100.0 }
    pub fn as_raw(&self) -> i16 { self.0 }
}

/// All measureable temperatures, but since
/// ome sensors may be unavailable, some temperatues may be missing
#[derive(Copy, Clone, Default)]
pub struct Temperatures {
    pub temps: [Option<Temperature>; 6],
}

impl Temperatures {
    pub fn get(&self, sensor: Sensor) -> Option<Temperature> {
        self.temps[usize::from(sensor)]
    }

    pub fn as_raw_with_default(&self, default: Temperature) -> [i16; 6] {
        [
            self.temps[0].unwrap_or(default).as_raw(),
            self.temps[1].unwrap_or(default).as_raw(),
            self.temps[2].unwrap_or(default).as_raw(),
            self.temps[3].unwrap_or(default).as_raw(),
            self.temps[4].unwrap_or(default).as_raw(),
            self.temps[5].unwrap_or(default).as_raw(),
        ]
    }

    pub fn replace_missing(&self, defaults: Temperatures) -> Temperatures {
        Temperatures {
            temps: [
                self.temps[0].or(defaults.temps[0]),
                self.temps[1].or(defaults.temps[1]),
                self.temps[2].or(defaults.temps[2]),
                self.temps[3].or(defaults.temps[3]),
                self.temps[4].or(defaults.temps[4]),
                self.temps[5].or(defaults.temps[5]),
            ]
        }
    }
}

impl fmt::Debug for Temperatures {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Oben: ")?;
        if self.temps[0].is_some() { write!(f, "{:.1} deg C ", self.temps[0].unwrap().0)?; }
        else { write!(f, "- ")?; }
        write!(f, "| Oberhalb:")?;
        if self.temps[1].is_some() { write!(f, "{:.1} deg C ", self.temps[1].unwrap().0)?; }
        else { write!(f, "- ")?; }
        write!(f, "| Mitte:")?;
        if self.temps[2].is_some() { write!(f, "{:.1} deg C ", self.temps[2].unwrap().0)?; }
        else { write!(f, "- ")?; }
        write!(f, "| Unterhalb:")?;
        if self.temps[3].is_some() { write!(f, "{:.1} deg C ", self.temps[3].unwrap().0)?; }
        else { write!(f, "- ")?; }
        write!(f, "| Unten:")?;
        if self.temps[4].is_some() { write!(f, "{:.1} deg C ", self.temps[4].unwrap().0)?; }
        else { write!(f, "- ")?; }
        write!(f, "| Außen:")?;
        if self.temps[5].is_some() { write!(f, "{:.1} deg C ", self.temps[5].unwrap().0)?; }
        else { write!(f, "- ")?; }
        Ok(())
    }
}


impl From<Vec<Result<Temperature, Error>>> for Temperatures {
    fn from(from: Vec<Result<Temperature, Error>>) -> Temperatures {
        Temperatures { temps : [
            from[0].as_ref().map(|f| *f).ok(), // Error -> None
            from[1].as_ref().map(|f| *f).ok(),
            from[2].as_ref().map(|f| *f).ok(),
            from[3].as_ref().map(|f| *f).ok(),
            from[4].as_ref().map(|f| *f).ok(),
            from[5].as_ref().map(|f| *f).ok(),
        ]}
    }
}


#[derive(Clone, Copy, Debug)]
/// Assigns the array elements from a measurement ([f64; 6]) to a sensor name
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

/// An async stream of [Option<f32>; 6] that provides new data every x seconds (see new())
pub struct SensorStream {
    trigger: Pin<Box<Interval>>,
    reader_future: Option<Pin<Box<dyn Future<Output=Vec<Result<Temperature, Error>>> + Send >>>,
    io_timeout: Duration
}

impl SensorStream {
    pub fn new(every: Duration) -> SensorStream {
        SensorStream {
            trigger: Box::pin(interval(every)),
            reader_future: None,
            io_timeout: every - Duration::from_millis(500),
        }
    }
}

impl Stream for SensorStream {
    type Item = Temperatures;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.reader_future.is_none() {
            // this will trigger every X seconds
            match self.trigger.as_mut().poll_next(cx) {
                Poll::Ready(Some(_)) => {
                    // Start the read procedure
                    self.reader_future = Some(read_all_temperatures(self.io_timeout).boxed());
                }
                Poll::Ready(None) => {
                    panic!("interval stream returned Ready(None)");
                    // return Poll::Ready(None); // should not happen
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }

        if let Some(reader_future) = &mut self.reader_future {
            match reader_future.as_mut().poll(cx) {
                Poll::Ready(temps) => {
                    self.reader_future = None;
                    return Poll::Ready(Some(Temperatures::from(temps)));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }

            }
        } else {
            panic!("Should not come here");
        }

    }
}


fn parse_temp(data: &str) -> Result<Temperature, Error> {
    let mut parts = data.split("t=");
    let _ = parts.next();
    if let Some(part2) = parts.next() {
        let temp_deg_c1000 = i32::from_str_radix(part2.trim(), 10)?;
        return Ok(Temperature::from_raw((temp_deg_c1000 / 10) as i16)); // the raw value has the unit degC*100
    }
    bail!("No `t=` in temperature data");
}

async fn read_temp_from_file(
    sensorname: &str,
    timeout: Duration,
) -> Result<Temperature, Error> {
    let data: String = async_std::io::timeout(timeout, async {
        let path = PathBuf::from(SENSOR_FILE_PATH).join(sensorname);
        let mut file = File::open(path).await?;
        let mut data = String::with_capacity(80);
        file.read_to_string(&mut data).await?;
        //info!("{}", &data);
        Ok(data)
    })
    .await?;

    Ok(parse_temp(&data).context(data)?)
}

async fn read_all_temperatures(timeout: Duration) -> Vec<Result<Temperature, Error>> {

    let read_futures = vec![
        read_temp_from_file("top", timeout),
        read_temp_from_file("higher", timeout),
        read_temp_from_file("mid", timeout),
        read_temp_from_file("lower", timeout),
        read_temp_from_file("bottom", timeout),
        read_temp_from_file("outside", timeout),
    ];

    join_all(read_futures.into_iter()).await
}


#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parsing() {
        let data = "5e 00 4b 46 7f ff 02 10 b0 : crc=b0 YES
                    5e 00 4b 46 7f ff 02 10 b0 t=5875
                    ";
        assert_eq!(parse_temp(data).unwrap().as_raw(), 587);
    }

}