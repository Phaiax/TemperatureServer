use std::boxed::Box;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use async_std::fs::File;
use async_std::path::PathBuf;
use async_std::prelude::*;
use async_std::stream::{interval, Interval};
use futures::future::join_all;
use futures::future::FutureExt;

use failure::{err_msg, bail, Error, Fail, ResultExt};
use log::info;

use crate::temp::{TemperatureStats, Temperatures};
use crate::SENSOR_FILE_PATH;

pub struct SensorStream {
    trigger: Pin<Box<Interval>>,
    reader_future: Option<Pin<Box<dyn Future<Output=Vec<Result<f32, Error>>>>>>,
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
    type Item = [Option<f32>; 6];

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
                    let temps = [
                        temps[0].as_ref().map(|f| *f).ok(), // Error -> None
                        temps[1].as_ref().map(|f| *f).ok(),
                        temps[2].as_ref().map(|f| *f).ok(),
                        temps[3].as_ref().map(|f| *f).ok(),
                        temps[4].as_ref().map(|f| *f).ok(),
                        temps[5].as_ref().map(|f| *f).ok(),
                    ];
                    return Poll::Ready(Some(temps));
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


fn parse_temp(data: &str) -> Result<f32, Error> {
    let mut parts = data.split("t=");
    let _ = parts.next();
    if let Some(part2) = parts.next() {
        let temp_int = i32::from_str_radix(part2.trim(), 10)?;
        let temp = (temp_int as f32) / 1000.0;
        return Ok(temp);
    }
    bail!("No `t=` in temperature data");
}

async fn read_temp_from_file(
    sensorname: &str,
    timeout: Duration,
) -> Result<f32, Error> {
    let data: String = async_std::io::timeout(timeout, async {
        let path = PathBuf::from(SENSOR_FILE_PATH).join(sensorname);
        let mut file = File::open(path).await?;
        let mut data = String::with_capacity(80);
        file.read_to_string(&mut data).await?;
        info!("{}", &data);
        Ok(data)
    })
    .await?;

    Ok(parse_temp(&data).context(data)?)
}

async fn read_all_temperatures(timeout: Duration) -> Vec<Result<f32, Error>> {

    let read_futures = vec![
        read_temp_from_file("top", timeout),
        read_temp_from_file("higher", timeout),
        read_temp_from_file("middle", timeout),
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
        assert_eq!(parse_temp(data).unwrap(), 5.875);
    }

}