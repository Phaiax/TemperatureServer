
use std::str::FromStr;
use std::fmt::{Display, Error as FmtError, Formatter};
use std::ops::Deref;

use serde::Serialize;
use serde::de::DeserializeOwned;
use failure::Error;

use filedb::{ChunkableData, ToFilenamePart};
use chrono::{Duration, NaiveDate, NaiveDateTime};
use chrono::prelude::*;

/// A wrapper for some structured data, that adds a timestamp and chunks the data by day into
/// the file_db.
#[derive(Serialize, Deserialize, Clone)]
pub struct Timestamped<D: Clone + Send + Sync + 'static> {
    #[serde(serialize_with = "naivedatetime_serde::naivedatetime_as_intint",
            deserialize_with = "naivedatetime_serde::intint_as_naivedatetime")]
    time: NaiveDateTime,
    data: D,
}

pub trait TimestampedMethods {
    fn date(&self) -> NaiveDate;
    fn time(&self) -> NaiveDateTime;
}

impl<D: Clone + Send + Sync + 'static> Timestamped<D> {
    pub fn now(data: D) -> Timestamped<D> {
        Timestamped {
            time: Local::now().naive_local(),
            data,
        }
    }

    pub fn at(time: NaiveDateTime, data: D) -> Timestamped<D> {
        Timestamped { time, data }
    }
}

impl<D: Clone + Send + Sync + 'static> TimestampedMethods for Timestamped<D> {
    fn date(&self) -> NaiveDate {
        self.time.date()
    }

    fn time(&self) -> NaiveDateTime {
        self.time
    }
}

impl<D: Clone + Send + Sync + 'static> Deref for Timestamped<D> {
    type Target = D;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}


#[derive(Hash, Ord, PartialOrd, Eq, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub struct NaiveDateWrapper(pub NaiveDate);

impl From<NaiveDateTime> for NaiveDateWrapper {
    fn from(datetime: NaiveDateTime) -> NaiveDateWrapper {
        NaiveDateWrapper(datetime.date())
    }
}

impl From<NaiveDate> for NaiveDateWrapper {
    fn from(datetime: NaiveDate) -> NaiveDateWrapper {
        NaiveDateWrapper(datetime)
    }
}

impl Display for NaiveDateWrapper {
    fn fmt(&self, f: &mut Formatter) -> Result<(), FmtError> {
        Display::fmt(&self.0, f)
    }
}

impl FromStr for NaiveDateWrapper {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(NaiveDateWrapper)
            .map_err(|parse_err| parse_err.into())
    }
}

impl ToFilenamePart for NaiveDateWrapper {
    fn to_filename_part(&self) -> String {
        format!("{}", self.0.format("%Y-%m-%d"))
    }
}


pub fn create_intervall_filtermap<
    D: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
    FM,
    R,
>(
    every: Duration,
    map: FM,
    capacity_multiplier: f32,
) -> Box<Fn(&[Timestamped<D>]) -> Vec<R> + Send + Sync + 'static>
where
    FM: Fn(&Timestamped<D>) -> R + Send + Sync + 'static,
{
    let closure = move |timestamped_slice: &[Timestamped<D>]| {
        let estimated_filtered_elements =
            (timestamped_slice.len() as f32 * capacity_multiplier) as usize + 3;
        let mut filtered: Vec<R> = Vec::with_capacity(estimated_filtered_elements);

        if timestamped_slice.len() >= 1 {
            let mut as_iter = timestamped_slice.iter();
            let first = as_iter.next().unwrap();
            let mut accept_time = (*first).key() + every;
            filtered.push(map(first));

            for next in as_iter {
                let curr_time = next.key();
                if curr_time >= accept_time {
                    accept_time = curr_time + every;
                    filtered.push(map(next));
                }
            }
        }
        filtered
    };
    Box::new(closure)
}

impl<D: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> ChunkableData
    for Timestamped<D> {
    type Key = NaiveDateTime;
    type ChunkKey = NaiveDateWrapper;
    fn chunk_key(&self) -> Self::ChunkKey {
        self.time.into()
    }
    fn key(&self) -> Self::Key {
        self.time
    }
    /// A hint for `Vec::with_capacity()`
    fn estimate_keys_per_chunk() -> usize {
        10000
    }
    /// A hint for `Vec::with_capacity()`
    fn estimate_serialized_bytes_per_chunk() -> usize {
        1_000_000
    }
}

//



// if let Some(caps) = CHUNK_FILENAME_REGEX.captures(&filename) {
//             let date = caps.get(1).unwrap().as_str();
//             let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
//             let control = Self::get_filepath(date, &path_base);
//             if control != path {
//                 bail!("Extracted {}, but path was {:?}", date, path);
//             }
//             files.push((control, date));
//         }


//         let mut data = Data {
//     time: Local::now().naive_local(),
//     mean: [0; 6],
//     celsius: ::temp::raw2celsius100(&data0.mean),
//     plug_state,
// };
// data.mean
//     .iter_mut()
//     .zip(data0.mean.iter())
//     .for_each(|(i, m)| *i = *m as u16);



pub mod naivedatetime_serde {

    use std::fmt::{Debug, Display, Error as FmtError, Formatter, Result as FmtResult};

    use serde::{Deserializer, Serializer};
    use serde::de::Error as SerdeError;
    use serde::de::{SeqAccess, Visitor};
    use serde::ser::SerializeTuple;

    use chrono::NaiveDateTime;



    pub fn naivedatetime_as_intint<S>(data: &NaiveDateTime, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut tup = ser.serialize_tuple(2)?;
        tup.serialize_element(&data.timestamp())?;
        tup.serialize_element(&data.timestamp_subsec_nanos())?;
        tup.end()
    }


    struct MySerdeError(String);

    impl SerdeError for MySerdeError {
        fn custom<T>(msg: T) -> Self
        where
            T: Display,
        {
            MySerdeError(format!("{}", msg))
        }
    }

    impl Display for MySerdeError {
        fn fmt(&self, f: &mut Formatter) -> Result<(), FmtError> {
            Display::fmt(&self.0, f)
        }
    }

    impl Debug for MySerdeError {
        fn fmt(&self, f: &mut Formatter) -> Result<(), FmtError> {
            Debug::fmt(&self.0, f)
        }
    }

    impl ::std::error::Error for MySerdeError {
        fn description(&self) -> &str {
            &self.0
        }
    }

    struct IntIntVisitor;
    impl<'de> Visitor<'de> for IntIntVisitor {
        type Value = NaiveDateTime;
        fn expecting(&self, formatter: &mut Formatter) -> FmtResult {
            write!(formatter, "two integers")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let secs = seq.next_element::<i64>()?
                .ok_or_else(|| SerdeError::invalid_length(0, &"two integers"))?;
            let nanos = seq.next_element::<i64>()?
                .ok_or_else(|| SerdeError::invalid_length(1, &"one more integer"))?;
            Ok(NaiveDateTime::from_timestamp(secs, nanos as u32))
        }
    }

    pub fn intint_as_naivedatetime<'de, D>(deser: D) -> Result<NaiveDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        deser.deserialize_tuple(2, IntIntVisitor)
    }

}
