
use std::collections::VecDeque;
use std::time::Duration;

use failure::Error;

use futures::Stream;
use futures::future::{Future, ok, err};
use tokio_io::codec::{Decoder, Encoder};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_core::reactor::Handle;
use tokio_serial::{Serial, SerialPortSettings, BaudRate, DataBits, FlowControl, Parity, StopBits,
                   SerialPort};

use bytes::BytesMut;

use temp::{Temperatures, TemperatureStats};
use SharedData;
use NANOEXT_SERIAL_DEVICE;

pub fn init_serial_port(serialhandle: Handle, shared : &SharedData) {

    let serialsetting = SerialPortSettings {
        baud_rate: BaudRate::Baud9600,
        data_bits: DataBits::Eight,
        flow_control: FlowControl::None,
        parity: Parity::None,
        stop_bits: StopBits::One,
        timeout: Duration::from_millis(1000),
    };

    let serial = Serial::from_path(NANOEXT_SERIAL_DEVICE,
                                   &serialsetting,
                                   &serialhandle).unwrap();


    let serial = serial.framed(SerialCodec::new());

    let shared_clone = shared.clone();
    let serialfuture = serial.for_each(move |ts| {
        println!("{:?}", ts.1);
        (*shared_clone).set(ts.1);
        ok(())
    }).or_else(|_e| err(()) );

    serialhandle.spawn(serialfuture);
}





struct SerialCodec {
    past : VecDeque<[u32; 6]>,
}

impl SerialCodec {
    fn new() -> SerialCodec {
        SerialCodec {
            past : VecDeque::with_capacity(100),
        }
    }
    fn sample(&mut self, sample : &Temperatures) {
        if self.past.len() == 100 {
            self.past.pop_front();
        }
        self.past.push_back(sample.raw.clone());
    }
    fn stats(&self) -> TemperatureStats {
        let cnt = self.past.len() as f64;
        let sums = self.past.iter().fold([0u64; 6], |mut acc, ts| {
            acc.iter_mut().zip(ts.iter()).for_each(|(acc_elem, raw)| *acc_elem += *raw as u64 ); acc
        });
        let mut res = TemperatureStats {
            mean : [0.; 6],
            std_dev : [0.; 6],
        };
        res.mean.iter_mut().zip(sums.iter()).for_each(|(m, s)| *m = *s as f64 / cnt);

        // sum( ( x_i - x_mean )² )

        let meandiffsquaredandsummed : [f64; 6] = self.past.iter().fold([0f64;6], |mut acc, ts| {
            acc.iter_mut().zip(ts.iter()).zip(res.mean.iter())
             .for_each(|((acc_elem, raw), mean)| *acc_elem += (*raw as f64 - mean).powi(2) );
            acc
        });
        res.std_dev.iter_mut().zip(meandiffsquaredandsummed.iter())
            .for_each(|(sdev, meansquaresum)| {
                *sdev = ( (1.0/cnt) * meansquaresum ).sqrt();
            });


        res

    }
}

impl Decoder for SerialCodec
{
    type Item = (Temperatures, TemperatureStats);
    type Error = Error;
    fn decode(
        &mut self,
        src: &mut BytesMut
    ) -> Result<Option<Self::Item>, Self::Error> {

        // find `;`
        if let Some(pos) = src.iter().position(|&b| b == b';') {
            let mut strdata = src.split_to(pos+1); // with `;`
            let l = strdata.len();
            strdata.truncate(l-1); // remove trailing `;`

            let strdata = String::from_utf8_lossy(&*strdata); // Cow
            let mut temperatures = Temperatures {
                raw : [0; 6]
            };
            let mut err = false;
            let num_temps : usize = strdata
                .split(',')
                .map(|ss| ss.split(':').last().unwrap() )
                .map(|s| s.parse().unwrap_or_else(|_| { err = true; 0 } ))
                .take(6)
                .enumerate()
                .map(|(i, n)| temperatures.raw[i] = n )
                .count();
            if num_temps != 6 || err {
                println!("err {}, {:?}", num_temps, temperatures);
                return Ok(None);
            }
            self.sample(&temperatures);
            return Ok(Some((temperatures, self.stats())));

        } else {
            return Ok(None);
        }

    }
}

impl Encoder for SerialCodec
{
    type Item = String;
    type Error = Error;
    fn encode(
        &mut self,
        _item: Self::Item,
        _dst: &mut BytesMut
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}