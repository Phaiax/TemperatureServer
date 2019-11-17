
use async_std::prelude::*;
use async_std::write;
use async_std::path::PathBuf;
use async_std::fs::File;
use std::fs::remove_file;
use std::os::unix::io::AsRawFd;
//use std::io::Write;

use log::{error};
use failure::{Error, bail};

use libc::c_int;
use libc::getpid;
use libc::EXIT_SUCCESS;
const EXIT_ERROR: c_int = -1 as c_int;

use libc::flock;
use libc::LOCK_EX;
use libc::LOCK_NB;

use libc::__errno_location;
use libc::EWOULDBLOCK;
use libc::EBADF;
use libc::EINTR;
use libc::EINVAL;
use libc::ENOLCK;

pub struct ExclusiveFilesystembasedLock {
    file: File,
    path: PathBuf,
}

enum FlockResult {
    LockPlaced,
    AlreadyLocked,
}

impl ExclusiveFilesystembasedLock {
    pub async fn try_set_lock(path: PathBuf) -> Result<ExclusiveFilesystembasedLock, Error> {
        if path.is_file().await {
            // At first, don't truncate the file, because we do not want to overwrite the pid.
            let file = File::open(&path).await?;
            match Self::place_lock(&file).await? {
                FlockResult::AlreadyLocked => bail!("Already locked"),
                FlockResult::LockPlaced => {}
            }
            // release file and lock, we want to truncate the file and write our own pid.
        }

        let mut file = File::create(&path).await?;
        match Self::place_lock(&file).await? {
            FlockResult::AlreadyLocked => bail!("Already locked. (Datarace?)"),
            FlockResult::LockPlaced => {}
        }
        Self::write_pid(&mut file).await?;

        Ok(ExclusiveFilesystembasedLock {
            file: file,
            path: path,
        })
    }

    async fn place_lock(file: &File) -> Result<FlockResult, Error> {
        let fd = file.as_raw_fd() as i32 as c_int;


        // Try to place an exclusive lock in nonblocking mode.
        // If successful, then flock==0.
        // If not successfull, then flock==-1 and errno=EWOULDBLOCK
        let flock = unsafe { flock(fd, LOCK_EX | LOCK_NB) };

        // EBADF  fd is not an open file descriptor.
        // EINTR  While waiting to acquire a lock, the call was interrupted
        //        by delivery of a signal caught by a handler; see signal(7).
        // EINVAL operation is invalid.
        // ENOLCK The kernel ran out of memory for allocating lock records.
        // EWOULDBLOCK The file is locked and the LOCK_NB flag was selected.


        match flock {
            EXIT_SUCCESS => Ok(FlockResult::LockPlaced),
            EXIT_ERROR => {
                let errno: c_int = unsafe { *__errno_location() };
                match errno {
                    EWOULDBLOCK => Ok(FlockResult::AlreadyLocked),
                    EBADF => bail!("Could not set database lock."),
                    EINTR => bail!("Could not set database lock."),
                    EINVAL => bail!("Could not set database lock."),
                    ENOLCK => bail!("Could not set database lock."),
                    _ => bail!("Unknown errno value after flock()."),
                }
            }
            _ => bail!("Unknown return value of flock()."),
        }
    }

    async fn write_pid(file: &mut File) -> Result<(), Error> {
        let pid = unsafe { getpid() } as i32;
        write!(file, "{}", pid).await?;
        file.sync_all().await?;
        Ok(())
    }
}

impl Drop for ExclusiveFilesystembasedLock {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd() as i32 as ::libc::c_int;

        // Try to remove the lock
        let flock = unsafe { ::libc::flock(fd, ::libc::LOCK_UN) };

        // We can only do logging here.
        match flock {
            EXIT_SUCCESS => {}
            EXIT_ERROR => {
                let errno: c_int = unsafe { *__errno_location() };
                match errno {
                    EWOULDBLOCK => error!("Could not remove database lock (EWOULDBLOCK)."),
                    EBADF => error!("Could not remove database lock (EBADF)."),
                    EINTR => error!("Could not remove database lock (EINTR)."),
                    EINVAL => error!("Could not remove database lock (EINVAL)."),
                    ENOLCK => error!("Could not remove database lock (ENOLCK)."),
                    _ => error!("Unknown errno value after flock()."),
                }
            }
            _ => {}
        }

        remove_file(&self.path).map_err(|e| error!("{}", e)).ok();
    }
}
