//!
//!
//! TLOG20 is a serial precision thermometer
//!
//!
//!
//!
//!

use std::collections::VecDeque;
use std::time::Duration;

use failure::{err_msg, Error, ResultExt};

use futures::Stream;
use futures::{future, Future};
use tokio_io::codec::{Decoder, Encoder};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_core::reactor::Handle;
use tokio_serial::{BaudRate, DataBits, FlowControl, Parity, Serial, SerialPort,
                   SerialPortSettings, StopBits};

use bytes::BytesMut;

use Shared;
use temp::{TemperatureStats, Temperatures};
use TLOG20_SERIAL_DEVICE;



pub fn init_serial_port(shared: Shared) -> Result<(), Error> {
    let serialsetting = SerialPortSettings {
        baud_rate: BaudRate::Baud4800,
        data_bits: DataBits::Eight,
        flow_control: FlowControl::None,
        parity: Parity::None,
        stop_bits: StopBits::One,
        timeout: Duration::from_millis(15000),
    };

    let serial = Serial::from_path(TLOG20_SERIAL_DEVICE, &serialsetting, &shared.handle())
        .context("TLOG20 not connected")?;

    let serial = serial.framed(Tlog20Codec::new());

    // `for_each` processes the `Stream` of decoded Tlog20Codec::Items (`f64`)
    // and returns a future that represents this processing until the end of time.
    // We can only spawn `Future<Item = (), Error = ()>`.
    let serialfuture = serial
        .for_each(|ts| {
            // New Item arrived
            info!("Reference: {:?}", ts);
            // This closure must return a future `Future<Item = (), Error = Tlog20Codec::Error>`.
            // `for_each` will run this future to completion before processing the next item.
            // But we can simply return an empty future.
            future::ok(())
        })
        .or_else(|_e| {
            // Map the error type to `()`, but at least print the error.
            error!("TLOG20 decoder error: {:?}", _e);
            future::err(())
        });

    shared.handle().spawn(serialfuture);

    Ok(())
}


///
/// This is the data:
///
///    $
///    @
///    I010110CDCFDA00080029
///    V0108404A
///    $
///
/// @: begin marker
/// $: end marker
/// I...: sensor identification
/// V...: sensor value, consists of
/// - `V`
/// - `01` ?
/// - `0840`: Temperature in hex and the unit 0.01 deg Celsius
/// - `4A`: Checksum of the line
struct Tlog20Codec {}

impl Tlog20Codec {
    fn new() -> Tlog20Codec {
        Tlog20Codec {}
    }
}


impl Decoder for Tlog20Codec {
    type Item = f64;
    type Error = Error;

    // Return Ok(None) if not ready,
    //        Ok(Some(item)) if an item was found
    //        Err(error) on disconnect or non recoverable error
    // Do not panic
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let pos_end = src.iter().position(|&b| b == b'$');
        if pos_end.is_none() {
            return Ok(None);
        }
        let pos_end = pos_end.unwrap();

        // extract from input buffer until first `$`, including `$`
        let buf = src.split_to(pos_end + 1);

        // contains a complete data set?
        let pos_begin = buf.iter().position(|&b| b == b'@');
        if pos_begin.is_none() {
            // Case: ` data $ @ data `
            info!("TLOG20: Case: ` data $ @ data `.");
            return Ok(None);
        }
        let pos_begin = pos_begin.unwrap();

        // Case: `$ @ data $`
        if pos_begin >= pos_end {
            info!("TLOG20: Case: ` $ @ data $ `.");
            return Ok(None); // first `$` has been cut from `src`
        }

        // cut trailing `$`
        let buf = &buf[pos_begin..buf.len() - 1];

        let as_str = String::from_utf8_lossy(buf); // Cow

        // extract temperature part
        let data_part = as_str.split('V').last().unwrap(); // never panics

        if data_part.len() < 6 {
            warn!("TLOG20: data error. `{}`", as_str);
            return Ok(None);
        }
        let temperature_in_hex = &data_part[2..6];

        // Convert to int
        let temperature = i32::from_str_radix(temperature_in_hex, 16);
        if temperature.is_err() {
            warn!("TLOG20: Could not convert to int. `{}`", as_str);
            return Ok(None);
        }
        let mut temperature = temperature.unwrap();

        // negative temperatures
        if temperature > 0x8000 {
            temperature = 0xffff - temperature;
        }

        // original value has unit `0.01 deg C`
        let temperature = temperature as f64 / 100.;
        return Ok(Some(temperature));
    }
}

impl Encoder for Tlog20Codec {
    type Item = String;
    type Error = Error;
    fn encode(&mut self, _item: Self::Item, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(())
    }
}
