
use std::collections::VecDeque;
use std::time::Duration;

use failure::{err_msg, Error, ResultExt};

use futures::Stream;
use futures::stream::SplitSink;
use futures::Sink;
use futures::{future, Future};
use futures::unsync::mpsc::Sender;

use tokio_io::codec::{Decoder, Encoder, Framed};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_core::reactor::Handle;
use tokio_serial::{BaudRate, DataBits, FlowControl, Parity, Serial, SerialPort,
                   SerialPortSettings, StopBits};

use bytes::BytesMut;
use bytes::buf::BufMut;

use temp::{TemperatureStats, Temperatures};
use Shared;
use NANOEXT_SERIAL_DEVICE;
use ErrorEvent;

pub type NanoextCommandSink = SplitSink<Framed<Serial, NanoextCodec>>;

/// `error_sink` can be used to respawn the serial handler
pub fn init_serial_port(shared: Shared) -> Result<NanoextCommandSink, Error>  {
    let serialsetting = SerialPortSettings {
        baud_rate: BaudRate::Baud9600,
        data_bits: DataBits::Eight,
        flow_control: FlowControl::None,
        parity: Parity::None,
        stop_bits: StopBits::One,
        timeout: Duration::from_millis(1000),
    };

    let serial = Serial::from_path(NANOEXT_SERIAL_DEVICE, &serialsetting, &shared.handle())
        .context("NANOEXT not connected")?;

    let serial = serial.framed(NanoextCodec::new());
    let (serial_write, serial_read) = serial.split();

    let shared_clone = shared.clone(); // for moving into closure
    let shared_clone2 = shared.clone(); // for moving into closure

    let mut every_i = 0u32;

    // `for_each` processes the `Stream` of decoded Tlog20Codec::Items (`f64`)
    // and returns a future that represents this processing until the end of time.
    // We can only spawn `Future<Item = (), Error = ()>`.
    let serialfuture = serial_read
        .for_each(move |ts| {
            // New Item arrived

            // Print every x item
            every_i += 1;
            if every_i == 10 {
                info!("{:?}", ts.1);
                every_i = 0;
            }

            // save into shared data
            shared_clone.temperatures.set(ts.1);

            // This closure must return a future `Future<Item = (), Error = Tlog20Codec::Error>`.
            // `for_each` will run this future to completion before processing the next item.
            // But we can simply return an empty future.
            future::ok(())
        })
        .or_else(move |_e| {
            // Map the error type to `()`, but at least print the error.
            error!("NANOEXT decoder error: {:?}", _e);
            // Will maybe spawn a new Nanoext
            shared_clone2.handle_error_async(ErrorEvent::NanoExtDecoderError);
            future::err(())
        });


    shared.handle().spawn(serialfuture.map_err(|_| ()));

    Ok(serial_write)
}


pub enum NanoExtCommand {
    PowerOn,
    PowerOff,
}


pub struct NanoextCodec {
    past: VecDeque<[u32; 6]>,
}

impl NanoextCodec {
    fn new() -> NanoextCodec {
        NanoextCodec {
            past: VecDeque::with_capacity(100),
        }
    }

    /// Save the last 100 datapoints
    fn sample(&mut self, sample: &Temperatures) {
        if self.past.len() == 100 {
            self.past.pop_front();
        }
        self.past.push_back(sample.raw.clone());
    }

    /// Calculate mean and standard derivation of the saved datapoints.
    ///
    /// All this is done in parallel for each of the 6 temperatures.
    fn stats(&self) -> TemperatureStats {
        let cnt = self.past.len() as f64;

        // Sum each sensors raw values into the accumulator
        let sums = self.past.iter().fold([0u64; 6], |mut acc, ts| {
            acc.iter_mut()
                .zip(ts.iter())
                .for_each(|(acc_elem, raw)| *acc_elem += *raw as u64);
            acc
        });


        let mut res = TemperatureStats {
            mean: [0.; 6],
            std_dev: [0.; 6],
        };

        // Mean
        res.mean
            .iter_mut()
            .zip(sums.iter())
            .for_each(|(m, s)| *m = *s as f64 / cnt);

        // sum( ( x_i - x_mean )² )

        // Calculate the sum part of the standard derivation into the accumulator
        let meandiffsquaredandsummed: [f64; 6] = self.past.iter().fold([0f64; 6], |mut acc, ts| {
            acc.iter_mut().zip(ts.iter()).zip(res.mean.iter()).for_each(
                |((acc_elem, raw), mean)| *acc_elem += (*raw as f64 - mean).powi(2),
            );
            acc
        });

        // multiply the sum by 1/n
        res.std_dev
            .iter_mut()
            .zip(meandiffsquaredandsummed.iter())
            .for_each(|(sdev, meansquaresum)| {
                *sdev = ((1.0 / cnt) * meansquaresum).sqrt();
            });


        res
    }
}

impl Decoder for NanoextCodec {
    type Item = (Temperatures, TemperatureStats);
    type Error = Error;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // find `;`
        let pos = src.iter().position(|&b| b == b';');
        if pos.is_none() {
            return Ok(None);
        }

        // extract from input buffer until first `;`, including `;`
        let buf = src.split_to(pos.unwrap() + 1);

        // remove trailing `;`
        let buf = &buf[0..buf.len() - 1];

        let strdata = String::from_utf8_lossy(buf); // Cow

        let mut temperatures = Temperatures { raw: [0; 6] };

        let mut err = false;
        let num_temps: usize = strdata
            .split(',')
            .map(|ss| ss.split(':').last().unwrap() /* split always delivers */ )
            .map(|s| s.parse().unwrap_or_else(|_| { err = true; 0 } ))
            .take(6) // does not panic
            .enumerate()
            .map(|(i, n)| temperatures.raw[i] = n )
            .count();

        if num_temps != 6 || err {
            error!(
                "Found {} instead of 6 temperatures: {:?}",
                num_temps,
                temperatures
            );
            return Ok(None);
        }

        self.sample(&temperatures);

        return Ok(Some((temperatures, self.stats())));
    }
}

impl Encoder for NanoextCodec {
    type Item = NanoExtCommand;
    type Error = Error;
    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            NanoExtCommand::PowerOn => dst.put_u8(b'1'),
            NanoExtCommand::PowerOff => dst.put_u8(b'0'),
        }
        Ok(())
    }
}
