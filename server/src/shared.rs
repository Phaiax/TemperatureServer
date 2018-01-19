
use nanoext::{NanoExtCommand, NanoextCommandSink};
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::time::Instant;
use temp::TemperatureStats;
use tokio_core::reactor::Handle;
use futures::{future, Future, Sink};
use futures::unsync::mpsc;
use failure::Error;
use Event;
use parameters::Parameters;
use MyFileDb;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PlugCommand {
    /// Use the trigger values defined in `SharedInner::parameters`
    Auto,
    ForceOn { until: Instant },
    ForceOff { until: Instant },
}

impl PlugCommand {
    pub fn check_elapsed(_self : &Cell<PlugCommand>) {
        let curr = _self.get();
        let new = match curr {
            PlugCommand::Auto => PlugCommand::Auto,
            PlugCommand::ForceOn { until } => {
                if until <= Instant::now() {
                    PlugCommand::Auto
                } else {
                    PlugCommand::ForceOn { until }
                }
            },
            PlugCommand::ForceOff { until } => {
                if until <= Instant::now() {
                    PlugCommand::Auto
                } else {
                    PlugCommand::ForceOff { until }
                }
            }
        };
        if new != curr {
            _self.set(new);
        }
    }

    pub fn is_auto(&self) -> bool {
        match self {
            &PlugCommand::Auto => true,
            _ => false,
        }
    }
}

// Todo: make newtype struct and create accessor methods for pub fields. Then remove shared argument
// in the member functions of SharedInner below. (do impl Shared instead)
pub type Shared = Rc<SharedInner>;

pub struct SharedInner {
    pub temperatures: Cell<TemperatureStats>,
    pub plug_state : Cell<bool>,
    pub plug_command : Cell<PlugCommand>,
    pub reference_temperature : Cell<Option<f64>>,
    event_sink: mpsc::Sender<Event>,
    pending_nanoext_command : RefCell<Option<NanoExtCommand>>,
    nanoext_command_sink: RefCell<Option<NanoextCommandSink>>,
    reactor_handle: RefCell<Option<Handle>>,
    pub nanoext_connected : Cell<bool>,
    pub tlog20_connected : Cell<bool>,
    db : MyFileDb,
    pub parameters : Parameters,
}

pub fn setup_shared(event_sink : mpsc::Sender<Event>, db : MyFileDb) -> Shared {

    Rc::new(SharedInner {
        temperatures: Cell::new(TemperatureStats::default()),
        event_sink,
        plug_state : Cell::new(false),
        plug_command : Cell::new(PlugCommand::Auto),
        reference_temperature : Cell::new(None),
        pending_nanoext_command : RefCell::new(None),
        nanoext_command_sink : RefCell::new(None),
        reactor_handle: RefCell::new(None),
        nanoext_connected: Cell::new(false),
        tlog20_connected: Cell::new(false),
        db,
        parameters : Parameters::default(),
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

    pub fn db(&self) -> &MyFileDb {
        &self.db
    }

    /// Asyncronously send a command to the nanoext.
    ///
    /// This will temporary comsume the sink and use the field `pending_nanoext_command`
    /// to save the next command while the sink is on its roundtrip for the current one.
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
