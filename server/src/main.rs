#![allow(unused_imports)]

extern crate hyper;
extern crate futures;
extern crate failure;
extern crate tokio_serial;
extern crate tokio_io;
extern crate bytes;
#[macro_use] extern crate lazy_static;


use std::collections::VecDeque;
use std::fmt::{Formatter, Debug};
use std::sync::RwLock;
use std::cell::Cell;
use std::rc::Rc;

use futures::future::{Future, ok, err};
use futures::Stream;

use hyper::header::ContentLength;
use hyper::server::{Http, Request, Response, Service};
use tokio_serial::{Serial, SerialPortSettings, BaudRate, DataBits, FlowControl, Parity, StopBits,
                   SerialPort};
use tokio_io::codec::{Decoder, Encoder};
use tokio_io::{AsyncRead, AsyncWrite};
use std::time::Duration;
use failure::Error;
use bytes::BytesMut;

type SharedData = Rc<Cell<TemperatureStats>>;

struct HelloWorld {
    shared : SharedData,
}

const PHRASE: &'static str = "Hello, World!";

impl Service for HelloWorld {
    // boilerplate hooking up hyper's server types
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    // The future representing the eventual Response your call will
    // resolve to. This can change to whatever Future you need.
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, _req: Request) -> Self::Future {
        // We're currently ignoring the Request
        // And returning an 'ok' Future, which means it's ready
        // immediately, and build a Response with the 'PHRASE' body.
        let shared = self.shared.get();
        let formatted = format!("{:?}", shared);

        Box::new(futures::future::ok(
            Response::new()
                .with_header(ContentLength(formatted.len() as u64))
                .with_body(formatted)
        ))
    }
}


fn raw2celsius_single(raw : f64, a : f64, b : f64, c : f64) -> f64 {
    let R : f64 = (raw * 100000.) / (1023. - raw);
    let Tinv = a + b * R.ln() + c * R.ln().powi(3);
    let T = (1./Tinv) - 273.15;
    return T;
}

fn raw2celsius(raw : &[f64 ; 6]) -> [f64; 6] {
    [
        raw2celsius_single(raw[0], 1.08067787e-02, -1.09703050e-03, 3.39993112e-06),
        raw2celsius_single(raw[1], 1.26655441e-02, -1.33464022e-03, 3.97068257e-06),
        raw2celsius_single(raw[2], 4.08499611e-02, -5.02859613e-03, 1.34083626e-05),
        raw2celsius_single(raw[3], 4.64316466e-02, -5.86985222e-03, 1.61191494e-05),
        raw2celsius_single(raw[4], 3.33607571e-02, -4.10284361e-03, 1.13271922e-05),
        raw2celsius_single(raw[5], 1.80743600e-02, -2.05474925e-03, 5.85871185e-06),
    ]
}

#[derive(Debug, Copy, Clone)]
struct Temperatures {
    raw : [u32; 6]
}

#[derive(Clone, Default, Copy)]
struct TemperatureStats {
    mean : [f64; 6],
    std_dev : [f64; 6],
}

impl Debug for TemperatureStats {
    fn fmt(&self, f: &mut Formatter) -> Result<(), ::std::fmt::Error> {
        let mean_celsius = raw2celsius(&self.mean);
        write!(f, "Means: {:.1}°C ({:.2}) ", mean_celsius[0], self.std_dev[0])?;
        write!(f, "{:.1}°C ({:.2}) ", mean_celsius[1], self.std_dev[1])?;
        write!(f, "{:.1}°C ({:.2}) ", mean_celsius[2], self.std_dev[2])?;
        write!(f, "{:.1}°C ({:.2}) ", mean_celsius[3], self.std_dev[3])?;
        write!(f, "{:.1}°C ({:.2}) ", mean_celsius[4], self.std_dev[4])?;
        write!(f, "{:.1}°C ({:.2}) ", mean_celsius[5], self.std_dev[5])?;
        Ok(())
    }
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


fn main() {
    let shared : SharedData = Rc::new(Cell::new(TemperatureStats::default()));

    let addr = "192.168.177.44:12345".parse().unwrap();
    let shared_clone = shared.clone();
    let server = Http::new().bind(&addr, move || Ok( HelloWorld { shared: shared_clone.clone() })).unwrap();

    let serialsetting = SerialPortSettings {
        baud_rate: BaudRate::Baud9600,
        data_bits: DataBits::Eight,
        flow_control: FlowControl::None,
        parity: Parity::None,
        stop_bits: StopBits::One,
        timeout: Duration::from_millis(1000),
    };
    let serialhandle = server.handle();
    let serial = Serial::from_path("/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0",
                                   &serialsetting,
                                   &serialhandle).unwrap();

    // use std::io::Read;
    // println!("BR: {:?}", serial.baud_rate());
    // for _i in [1,2,3].iter() {
    //     let mut buf = Vec::with_capacity(100);
    //     let bytes_read = serial.read(&mut buf).unwrap();
    //     println!("Read {}: {:?}", bytes_read, buf);
    // }

    // reset MC
    //serial.write_data_terminal_ready(true).unwrap();
    //serial.write_data_terminal_ready(false).unwrap();

    let serial = serial.framed(SerialCodec::new());

    let shared_clone = shared.clone();
    let serialfuture = serial.for_each(move |ts| {
        println!("{:?}", ts.1);
        (*shared_clone).set(ts.1);
        ok(())
    }).or_else(|_e| err(()) );

    serialhandle.spawn(serialfuture);


    server.run().unwrap();

    // serial.into_inner().shutdown().unwrap();
}