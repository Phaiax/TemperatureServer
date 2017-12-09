
use nanoext::{NanoExtCommand, NanoextCommandSink};
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use temp::TemperatureStats;
use tokio_core::reactor::Handle;
use futures::{future, Future, Sink};
use futures::unsync::mpsc;
use failure::Error;

use Event;


pub type Shared = Rc<SharedInner>;

pub struct SharedInner {
    pub temperatures: Cell<TemperatureStats>,
    event_sink: mpsc::Sender<Event>,
    pending_nanoext_command : RefCell<Option<NanoExtCommand>>,
    nanoext_command_sink: RefCell<Option<NanoextCommandSink>>,
    reactor_handle: RefCell<Option<Handle>>,
}

pub fn setup_shared(event_sink : mpsc::Sender<Event>) -> Shared {

    Rc::new(SharedInner {
        temperatures: Cell::new(TemperatureStats::default()),
        event_sink,
        pending_nanoext_command : RefCell::new(None),
        nanoext_command_sink : RefCell::new(None),
        reactor_handle: RefCell::new(None),
    })
}

impl SharedInner {
    pub fn handle_event_async(&self, e: Event) {
        let send_and_flush = self.event_sink
            .clone()
            .send(e)
            .map(|_event_sink| ())
            .map_err(|_e| {
                error!("Could not handle error in an async fashion.");
                ()
            });
        self.spawn(send_and_flush);
    }

    pub fn handle(&self) -> Handle {
        self.reactor_handle.borrow()
            .as_ref()
            .expect("Called handle() before handle was ready.")
            .clone()
    }

    pub fn spawn<F>(&self, f: F)
    where
        F: Future<Item = (), Error = ()> + 'static,
    {
        self.reactor_handle.borrow()
            .as_ref()
            .expect("Called handle() before handle was ready.")
            .spawn(f);
    }

    // Once the core is up, put the handle here.
    pub fn put_handle(&self, handle: Handle) {
        let mut option = self.reactor_handle.borrow_mut();
        if option.is_some() {
            panic!("Attempt to put reactor_handle into Shared twice.");
        }
        ::std::mem::replace(&mut *option, Some(handle));
    }

    /// If a new serial connection is created, put the corresponding sink here,
    /// so we have a place where to dispatch commands
    pub fn put_command_sink(&self, command_sink: NanoextCommandSink, shared : Shared) {
        // TODO: via take and get_or_insert
        let mut option = self.nanoext_command_sink.borrow_mut();
        let _maybe_old_sink = ::std::mem::replace(&mut *option, Some(command_sink));
        if let Some(next) = self.pending_nanoext_command.borrow_mut().take() {
            self.send_command_async(next, shared.clone());
        }
    }

    /// Asyncronously send a command to the nanoext.
    ///
    /// This will temporary comsume the sink and use the `pending_nanoext_command`
    /// to save the command while the sink is on its roundtrip.
    ///
    /// If another sink appeared when the original sink comes home (connection reset),
    /// then redo the command.
    pub fn send_command_async(&self, cmd : NanoExtCommand, shared : Shared) {
        match self.nanoext_command_sink.borrow_mut().take() {
            Some(sink) => {
                let send_and_return_home = sink.send(cmd).and_then(move |sink| {
                    // Put the `sink` to `shared` if the spot is still empty.
                    // I don't know if it can happen that the spot is already filled
                    // by a new connection. If the spot is filled, I would expect
                    // that the write on the old sink (that one we just got back) had failed.
                    // Then we would enter the `or_else` part.
                    if shared.nanoext_command_sink.borrow_mut().take().is_some() {
                        debug!("Successful send on old sink, but new sink present.");
                        // TODO: redo command! (should we use timestamps to be able to reject
                        // old commands?)
                    } else {
                        shared.nanoext_command_sink.borrow_mut().get_or_insert(sink);
                        // Trigger pending command
                        if let Some(next) = shared.pending_nanoext_command.borrow_mut().take() {
                            shared.send_command_async(next, shared.clone());
                        }
                    }
                    future::ok(())
                }).or_else(|error : Error| {
                    error!("Could not send command to Nanoext. ({})", error);
                    // We should trigger the next command, but we do not have a sink.
                    // The `put_command_sink` command will take care of a pending command.
                    future::err(())
                });
                self.spawn(send_and_return_home);
            },
            None => {
                // Put `cmd` into pending
                self.pending_nanoext_command.borrow_mut().take(); // clear old
                self.pending_nanoext_command.borrow_mut().get_or_insert(cmd);
            }
        }
    }
}
