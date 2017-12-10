#![allow(dead_code, unused_variables)]

use std::env;
use std::thread::{spawn, Thread};
use std::fmt::Display;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, MutexGuard};
use std::sync::Arc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs::{read_dir, File};
use std::io::{BufReader, BufWriter};
use std::io::prelude::*;

use futures::{future, Future};

use futures_cpupool::{CpuPool, CpuFuture};

use failure::{err_msg, Error, ResultExt};

use temp::{TemperatureStats, Temperatures, raw2celsius};

use chrono::NaiveDateTime;
use chrono::prelude::*;

use rmps::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};

use regex::Regex;

#[derive(Serialize, Deserialize, Clone)]
pub struct Data {
    pub time: NaiveDateTime,
    pub mean: [f64; 6],
    pub celsius: [f64; 6],
}


// Chunks are days
struct Chunk {
    path: PathBuf,
    /// This data is always newer that the disk data, except if data is None
    /// also: Data is always sorted
    data: Option<Arc<Vec<Data>>>,
    /// If Some, then this is on par with data
    serialized: Option<Arc<Vec<u8>>>,
    disk_is_up_to_date: bool,
}

impl Chunk {
    fn new(p: PathBuf) -> Chunk {
        Chunk {
            path: p,
            data: Some(Arc::new(vec![])),
            serialized: None,
            disk_is_up_to_date: false,
        }
    }

    fn load(p: PathBuf) -> Chunk {
        Chunk {
            path: p,
            data: None,
            serialized: None,
            disk_is_up_to_date: true,
        }
    }

    fn force_to_memory(&mut self) -> Result<(), Error> {
        if self.data.is_none() && self.disk_is_up_to_date {
            let file = File::open(&self.path)?;
            let size = file.metadata().map(|m| m.len()).unwrap_or(100000);
            let mut buf_reader = BufReader::new(file);
            let mut contents = Vec::with_capacity(size as usize);
            buf_reader.read_to_end(&mut contents)?;

            let mut de = Deserializer::new(&contents[..]);
            let desered: Vec<Data> = Deserialize::deserialize(&mut de).unwrap();
            self.data = Some(Arc::new(desered));
        } else if self.data.is_some() {
            // ok
        } else {
            bail!(
                "No data, but disk not up to date!? ({})",
                self.path.to_string_lossy()
            );
        }
        Ok(())
    }

    fn sync_to_disk(&mut self) -> Result<(), Error> {
        if !self.disk_is_up_to_date {
            if self.data.is_none() {
                bail!("No data to write to {}", self.path.to_string_lossy());
            }

            self.sync_serialized()?;

            let mut file = File::open(&self.path)?;
            file.write_all(&self.serialized.as_ref().unwrap()[..])?;

            self.disk_is_up_to_date = true;
        }
        Ok(())
    }

    fn sync_serialized(&mut self) -> Result<(), Error> {
        if self.data.is_some() && self.serialized.is_none() {
            let mut buf = Vec::with_capacity(0);
            self.data
                .as_ref()
                .unwrap()
                .serialize(&mut Serializer::new(&mut buf))
                .unwrap();
            self.serialized = Some(Arc::new(buf));
        };
        Ok(())
    }

    pub fn update<F, R>(&mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(&mut Vec<Data>) -> R,
    {
        self.update_no_sort(|mut data| {
            let r = f(&mut data);
            data.sort_by_key(|data| data.time);
            r
        })
    }

    fn update_no_sort<F, R>(&mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(&mut Vec<Data>) -> R,
    {
        self.force_to_memory()?;
        self.serialized = None;
        self.disk_is_up_to_date = false;
        if let Some(mut data) = Arc::get_mut(self.data.as_mut().unwrap()) {
            // there was only one owner
            return Ok(f(&mut data));
        }
        use std::ops::Deref;
        let mut cloned_vec: Vec<Data> = self.data.as_ref().unwrap().deref().clone();
        let r = f(&mut cloned_vec);
        self.data = Some(Arc::new(cloned_vec));
        Ok(r)
    }

    pub fn get_shared_vec(&mut self) -> Result<Arc<Vec<Data>>, Error> {
        self.force_to_memory()?;

        Ok(self.data.as_ref().unwrap().clone())
    }

    pub fn get_serialized(&mut self) -> Result<Arc<Vec<u8>>, Error> {
        self.force_to_memory()?;
        self.sync_serialized()?;

        Ok(self.serialized.as_ref().unwrap().clone())
    }

    pub fn get_by_key(&mut self, key: NaiveDateTime) -> Result<Option<Data>, Error> {
        self.force_to_memory()?;

        let data = &*self.data.as_ref().unwrap();

        let pos = data.binary_search_by_key(&key, |data| data.time);

        match pos {
            Ok(index) => return Ok(data.get(index).cloned()),
            Err(_would_be_insert) => Ok(None),
        }
    }

    pub fn insert_or_update(&mut self, data: Data) -> Result<(), Error> {
        self.update_no_sort(|vec| {
            let pos = vec.binary_search_by_key(&data.time, |data| data.time);
            match pos {
                Ok(index) => vec[index] = data,
                Err(would_be_insert) => vec.insert(would_be_insert, data),
            }
        })
    }
}



type Chunks = HashMap<NaiveDate, Arc<Mutex<Chunk>>>;

// Filedb stores chunked data into a folder addressable by time and date
pub struct FileDb {
    chunks: Arc<Mutex<Chunks>>,
    path_base: PathBuf,
    pool: CpuPool,
}


impl FileDb {
    pub fn establish_connection() -> Result<FileDb, Error> {
        let database_url =
            env::var("LOG_FOLDER").context("Environment variable LOG_FOLDER must be set.")?;

        let pool = CpuPool::new(2);

        let mut chunks = HashMap::new();
        for (path, existing_date) in Self::get_existing_files(Path::new(&database_url))? {
            chunks.insert(existing_date, Arc::new(Mutex::new(Chunk::load(path))));
        }

        Ok(FileDb {
            pool,
            chunks: Arc::new(Mutex::new(chunks)),
            path_base: database_url.into(),
        })
    }

    fn get_filepath<P: AsRef<Path>>(date: NaiveDate, path_base: P) -> PathBuf {
        path_base
            .as_ref()
            .join(Path::new(&format!("chunk-{}.db", date.format("%Y-%m-%d"))))
    }

    fn get_existing_files<P: AsRef<Path>>(
        path_base: P,
    ) -> Result<Vec<(PathBuf, NaiveDate)>, Error> {
        lazy_static! {
            static ref CHUNK_FILENAME_REGEX : Regex =
                Regex::new(r"chunk-([0-9]{4}-[0-9]{2}-[0-9]{2}).db").unwrap();
        }
        let mut files = vec![];
        for entry in read_dir(&path_base).context("LOG_FOLDER is not a directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let filename = path.file_name()
                    .expect("File has no filename?!")
                    .to_string_lossy();

                if let Some(caps) = CHUNK_FILENAME_REGEX.captures(&filename) {
                    let date = caps.get(1).unwrap().as_str();
                    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
                    let control = Self::get_filepath(date, &path_base);
                    if control != path {
                        bail!("Extracted {}, but path was {:?}", date, path);
                    }
                    files.push((control, date));
                }
            }
        }
        Ok(files)
    }

    fn get_or_create_by_date<P: AsRef<Path>>(
        this: &Arc<Mutex<Chunks>>,
        date: NaiveDate,
        path_base: P,
    ) -> Arc<Mutex<Chunk>> {
        // chunks is a Hasmap
        // chunk is a vector
        let mut chunks: MutexGuard<Chunks> = this.lock().unwrap();
        if chunks.contains_key(&date) {
            chunks.get(&date).unwrap().clone()
        } else {
            let new_chunk = Arc::new(Mutex::new(
                Chunk::new(Self::get_filepath(date, &path_base)),
            ));
            chunks.insert(date, Arc::clone(&new_chunk));
            new_chunk
        }
    }

    pub fn save_all(&self) -> Result<(), Error> {
        let chunks = self.chunks.lock().unwrap();
        for (ref date, ref chunk) in (*chunks).iter() {
            chunk.lock().unwrap().sync_to_disk()?;
        }
        Ok(())
    }


    fn result_to_future<T, E : Display>(res : Result<T, E>) -> future::FutureResult<T, E> {
        match res {
            Ok(d) => future::ok::<T,E>(d),
            Err(e) => {
                error!("DB Error {}", e);
                future::err::<T,E>(e)
            }
        }
    }

    pub fn insert_or_update_async(&self, data: TemperatureStats) -> CpuFuture<Data, Error> {

        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();
        let data = Data {
            time: Local::now().naive_local(),
            mean: data.mean,
            celsius: raw2celsius(&data.mean),
        };

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let date = data.time.date();
            let chunk = Self::get_or_create_by_date(&chunks, date, path_base);
            let r = chunk.lock().unwrap().insert_or_update(data.clone()).map(|_| data);
            Self::result_to_future(r)
        }));

        self.pool.spawn(f)
        // forget() keeps the pool running but forgets the CpuFuture
    }

    pub fn get_serialized_by_date(&self, date : NaiveDate) -> CpuFuture<Arc<Vec<u8>>, Error> {

        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_date(&chunks, date, path_base);
            let r = chunk.lock().unwrap().get_serialized();
            Self::result_to_future(r)
        }));

        self.pool.spawn(f)
    }


    pub fn get_by_date(&self, date : NaiveDate) -> CpuFuture<Arc<Vec<Data>>, Error> {

        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_date(&chunks, date, path_base);
            let r = chunk.lock().unwrap().get_shared_vec();
            Self::result_to_future(r)
        }));

        self.pool.spawn(f)
    }

    pub fn get_by_datetime(&self, time : NaiveDateTime) -> CpuFuture<Option<Data>, Error> {

        let chunks = self.chunks.clone();
        let path_base = self.path_base.clone();

        let f: Box<Future<Item = _, Error = _> + Send> = Box::new(future::lazy(move || {
            let chunk = Self::get_or_create_by_date(&chunks, time.date(), path_base);
            let r = chunk.lock().unwrap().get_by_key(time);
            Self::result_to_future(r)
        }));

        self.pool.spawn(f)
    }

}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_db() {
        ::dotenv::dotenv().ok();
        let db = FileDb::establish_connection().unwrap();

        let temp = TemperatureStats {
            mean: [500., 600., 500., 400., 500., 700.],
            std_dev: [0., 0., 0., 0., 0., 0.],
        };

        let pooloperation = db.insert_or_update_async(temp);
        let data = pooloperation.wait().unwrap();

        assert!(data.mean == temp.mean);

    }
}