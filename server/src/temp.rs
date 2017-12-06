
use std::fmt::{Formatter, Debug};

#[allow(non_snake_case)]
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
pub struct Temperatures {
    pub raw : [u32; 6]
}

#[derive(Clone, Default, Copy)]
pub struct TemperatureStats {
    pub mean : [f64; 6],
    pub std_dev : [f64; 6],
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