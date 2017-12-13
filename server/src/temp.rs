
use std::fmt::{Debug, Formatter};

#[allow(non_snake_case)]
fn raw2celsius_single(raw: f64, a: f64, b: f64, c: f64) -> f64 {
    let R: f64 = (raw * 100000.) / (1023. - raw);
    let Tinv = a + b * R.ln() + c * R.ln().powi(3);
    let T = (1. / Tinv) - 273.15;
    return T;
}


pub fn raw2celsius(raw: &[f64; 6]) -> [f64; 6] {
    [
        raw2celsius_single(raw[0], 7.05798867e-03, -6.30287025e-04, 2.32328933e-06),
        raw2celsius_single(raw[1], 5.62647076e-03, -4.66253561e-04, 2.00605696e-06),
        raw2celsius_single(raw[2], 2.83330737e-02, -3.42368415e-03, 9.48931324e-06),
        raw2celsius_single(raw[3], 4.90349955e-02, -6.16875325e-03, 1.66562829e-05),
        raw2celsius_single(raw[4], 4.70466017e-02, -5.83030411e-03, 1.53853008e-05),
        raw2celsius_single(raw[5], 2.07942145e-02, -2.39393274e-03, 6.63539211e-06),
    ]
}

pub fn raw2celsius100(raw: &[f64; 6]) -> [i16; 6] {
    [
        (raw2celsius_single(raw[0], 7.05798867e-03, -6.30287025e-04, 2.32328933e-06) * 100. ) as i16 ,
        (raw2celsius_single(raw[1], 5.62647076e-03, -4.66253561e-04, 2.00605696e-06) * 100. ) as i16 ,
        (raw2celsius_single(raw[2], 2.83330737e-02, -3.42368415e-03, 9.48931324e-06) * 100. ) as i16 ,
        (raw2celsius_single(raw[3], 4.90349955e-02, -6.16875325e-03, 1.66562829e-05) * 100. ) as i16 ,
        (raw2celsius_single(raw[4], 4.70466017e-02, -5.83030411e-03, 1.53853008e-05) * 100. ) as i16 ,
        (raw2celsius_single(raw[5], 2.07942145e-02, -2.39393274e-03, 6.63539211e-06) * 100. ) as i16 ,
    ]
}

#[derive(Debug, Copy, Clone)]
pub struct Temperatures {
    pub raw: [u32; 6],
}

#[derive(Clone, Default, Copy)]
pub struct TemperatureStats {
    pub mean: [f64; 6],
    pub std_dev: [f64; 6],
}

impl Debug for TemperatureStats {
    fn fmt(&self, f: &mut Formatter) -> Result<(), ::std::fmt::Error> {
        let mean_celsius = raw2celsius(&self.mean);
        //write!(f, "[ {:3.1}, ", self.mean[0])?;
        //write!(f, "{:3.1}, ", self.mean[1])?;
        //write!(f, "{:3.1}, ", self.mean[2])?;
        //write!(f, "{:3.1}, ", self.mean[3])?;
        //write!(f, "{:3.1}, ", self.mean[4])?;
        //write!(f, "{:3.1} ] (std:{:.1}) ", self.mean[5], self.std_dev[0])?;
        // write!(f, "Means: {:.1}°C ({:.1}, {:.1}) ",
        // mean_celsius[0], self.mean[0], self.std_dev[0])?;
         write!(f, "Oben: {:.1} deg C ", mean_celsius[0])?;
         write!(f, "| Oberhalb: {:.1} deg C ", mean_celsius[1])?;
         write!(f, "| Mitte: {:.1} deg C ", mean_celsius[2])?;
         write!(f, "| Unterhalb: {:.1} deg C ", mean_celsius[3])?;
         write!(f, "| Unten: {:.1} deg C ", mean_celsius[4])?;
         write!(f, "| Außen: {:.1} deg C ", mean_celsius[5])?;
        Ok(())
    }
}
