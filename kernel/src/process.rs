///! Process

use stack::KernelStack;
use i386::process_switch::*;
use i386::mem::paging::InactivePageTables;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use event::Waitable;
use spin::{RwLock, RwLockWriteGuard};
use sync::SpinLockIRQ;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::fmt::{self, Debug};
use scheduler;
use error::Error;

/// The struct representing a process. There's one for every process.
///
/// It contains many information about the process :
///
/// - Its type (regular userspace process, or kworker)
/// - Its state (Running, Scheduled, Stopped)
/// - Its memory pages
/// - Its kernel stack, for syscalls and interrupts
/// - Its hardware context, to be restored on rescheduling
#[derive(Debug)]
pub struct ProcessStruct {
    pub name:                 String,
    pub pstate:               ProcessStateAtomic,
    pub pmemory:              SpinLockIRQ<ProcessMemory>,
    pub pstack:               KernelStack,
    pub phwcontext:           SpinLockIRQ<ProcessHardwareContext>,
    pub phandles:             SpinLockIRQ<HandleTable>,

    /// Interrupt disable counter.
    ///
    /// # Description
    ///
    /// Allows recursively disabling interrupts while keeping a sane behavior.
    /// Should only be manipulated through sync::enable_interrupts and
    /// sync::disable_interrupts.
    ///
    /// Used by the SpinLockIRQ to implement recursive irqsave logic.
    pub pint_disable_counter: AtomicUsize,

    /// A vector of readable IO ports.
    ///
    /// When task switching, the IOPB will be changed to take this into account.
    // TODO: This is i386-specific. Sucks, but it should *really* go somewhere else.
    // Maybe in ProcessMemory?
    pub ioports: Vec<u16>
}

#[derive(Debug)]
pub enum Handle {
    ReadableEvent(Box<Waitable>),
}

#[derive(Debug)]
pub struct HandleTable {
    table: BTreeMap<u32, Arc<Handle>>,
    counter: u32
}

impl HandleTable {
    pub fn new() -> HandleTable {
        HandleTable {
            table: BTreeMap::new(),
            counter: 1
        }
    }

    pub fn add_handle(&mut self, handle: Arc<Handle>) -> u32 {
        loop {
            let handlenum = self.counter;
            self.counter += 1;
            if !self.table.contains_key(&handlenum) {
                self.table.insert(handlenum, handle);
                break handlenum;
            }
        }
    }

    pub fn get_handle(&self, handle: u32) -> Arc<Handle> {
        self.table[&handle].clone()
    }
}

/// Just a handy shortcut
pub type ProcessStructArc = Arc<ProcessStruct>;

/// The state of a process.
///
/// - Running: currently on the CPU
/// - Scheduled: scheduled to be running
/// - Stopped: not in the scheduled queue, waiting for an event
/// - Readying: In the process of getting ready. Should go to the Stopped state soon.
/// - NotReady: never added to the schedule queue yet. Should be started with `scheduler::start_process`
///
/// Since SMP is not supported, there is only one Running process.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum ProcessState {
    Running = 0,
    Scheduled = 1,
    Stopped = 2,
    Readying = 3,
    NotReady = 4,
    Killed = 5,
}

impl ProcessState {
    fn from_primitive(v: usize) -> ProcessState {
        match v {
            0 => ProcessState::Running,
            1 => ProcessState::Scheduled,
            2 => ProcessState::Stopped,
            3 => ProcessState::Readying,
            4 => ProcessState::NotReady,
            5 => ProcessState::Killed,
            _ => panic!("Invalid process state"),
        }
    }
}

pub struct ProcessStateAtomic(AtomicUsize);

impl Debug for ProcessStateAtomic {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(&self.load(Ordering::SeqCst), f)
    }
}

impl ProcessStateAtomic {
    pub fn new(state: ProcessState) -> ProcessStateAtomic {
        ProcessStateAtomic(AtomicUsize::new(state as usize))
    }

    pub fn into_inner(self) -> ProcessState {
        ProcessState::from_primitive(self.0.into_inner())
    }

    pub fn load(&self, order: Ordering) -> ProcessState {
        ProcessState::from_primitive(self.0.load(order))
    }

    pub fn store(&self, val: ProcessState, order: Ordering) {
        self.0.store(val as usize, order)
    }

    pub fn swap(&self, val: ProcessState, order: Ordering) -> ProcessState {
        ProcessState::from_primitive(self.0.swap(val as usize, order))
    }

    pub fn compare_and_swap(&self, current: ProcessState, new: ProcessState, order: Ordering) -> ProcessState {
        ProcessState::from_primitive(self.0.compare_and_swap(current as usize, new as usize, order))
    }

    pub fn compare_exchange(&self, current: ProcessState, new: ProcessState, success: Ordering, failure: Ordering) -> Result<ProcessState, ProcessState> {
        self.0.compare_exchange(current as usize, new as usize, success, failure)
            .map(ProcessState::from_primitive)
            .map_err(ProcessState::from_primitive)
    }

    pub fn compare_exchange_weak(&self, current: ProcessState, new: ProcessState, success: Ordering, failure: Ordering) -> Result<ProcessState, ProcessState> {
        self.0.compare_exchange_weak(current as usize, new as usize, success, failure)
            .map(ProcessState::from_primitive)
            .map_err(ProcessState::from_primitive)
    }

    pub fn fetch_update<F>(&self, mut f: F, fetch_order: Ordering, set_order: Ordering) -> Result<ProcessState, ProcessState>
    where
        F: FnMut(ProcessState) -> Option<ProcessState>
    {
        self.0.fetch_update(|v| f(ProcessState::from_primitive(v)).map(|v| v as usize),
                            fetch_order, set_order)
            .map(ProcessState::from_primitive)
            .map_err(ProcessState::from_primitive)
    }
}

/// The memory pages of this process
///
/// - Inactive contains the process's pages.
/// - Active means the already currently active ones, accessible through ACTIVE_PAGE_TABLES.
///
/// A ProcessMemory should be the only owner of a process' pages
#[derive(Debug)]
pub enum ProcessMemory {
    Inactive(SpinLockIRQ<InactivePageTables>),
    Active
}

impl ProcessStruct {
    /// Creates a new process.
    pub fn new(name: String, ioports: Vec<u16>) -> ProcessStructArc {
        use ::core::mem::ManuallyDrop;

        // allocate its memory space
        let pmemory = SpinLockIRQ::new(ProcessMemory::Inactive(SpinLockIRQ::new(InactivePageTables::new())));

        // allocate its kernel stack
        let pstack = KernelStack::allocate_stack()
            .expect("Couldn't allocate a kernel stack");

        // hardware context will be computed later in this function, write a dummy value for now
        let empty_hwcontext = SpinLockIRQ::new(ProcessHardwareContext::new());

        // the state of the process, NotReady
        let pstate = ProcessStateAtomic::new((ProcessState::NotReady));

        let p = Arc::new(
            ProcessStruct {
                name,
                pstate,
                pmemory,
                pstack,
                phwcontext : empty_hwcontext,
                phandles: SpinLockIRQ::new(HandleTable::new()),
                pint_disable_counter: AtomicUsize::new(0),
                ioports
            }
        );

        p
    }

    /// Creates the very first process at boot.
    ///
    /// # Safety
    ///
    /// Use only for creating the very first process. Should never be used again after that.
    /// Must be using a valid KernelStack, a valid ActivePageTables.
    ///
    /// # Panics
    ///
    /// ThreadInfoInStack will be initialized, it must not already have been
    pub unsafe fn create_first_process() -> Arc<ProcessStruct> {

        // the state of the process, currently running
        let pstate = ProcessStateAtomic::new(ProcessState::Running);

        // use the already allocated stack
        let pstack = KernelStack::get_current_stack();

        // the saved esp will be overwritten on schedule-out anyway
        let phwcontext = SpinLockIRQ::new(ProcessHardwareContext::new());

        // the already currently active pages
        let pmemory = SpinLockIRQ::new(ProcessMemory::Active);

        let p = Arc::new(
            ProcessStruct {
                name: String::from("init"),
                pstate,
                pmemory,
                pstack,
                phwcontext,
                phandles: SpinLockIRQ::new(HandleTable::new()),
                pint_disable_counter: AtomicUsize::new(0),
                ioports: Vec::new(),
            }
        );

        p
    }

    /// Sets the entrypoint and userspace stack pointer. Puts the Process in Stopped state.
    ///
    /// If the process is not in the NotReady state, it will fail with ProcessAlreadyStarted.
    ///
    /// # Safety
    ///
    /// The given entrypoint *must* point to a mapped address in that process's address space.
    /// The function makes no attempt at checking if it is kernel or userspace.
    pub unsafe fn set_start_arguments(&self, ep: usize, stack: usize) -> Result<(), Error> {
        let oldval = self.pstate.compare_and_swap(ProcessState::NotReady, ProcessState::Readying, Ordering::SeqCst);

        if oldval != ProcessState::NotReady {
            return Err(Error::ProcessAlreadyStarted);
        }

        // prepare the process's stack for its first schedule-in
        unsafe {
            // Safety: With the compare_and_swap above, we ensure that this can only
            // be run exactly *once*.
            // Furthermore, since we're in readying state (and were in NotReady before),
            // we can ensure that we have never been scheduled, and cannot be scheduled.
            prepare_for_first_schedule(self, ep, stack);
        }

        self.pstate.store(ProcessState::Stopped, Ordering::SeqCst);
        Ok(())
    }

    /// Sets the process to the Killed state.
    ///
    /// We reschedule the process (cancelling any waiting it was doing).
    /// In this state, the process will die when attempting to return to userspace.
    pub fn kill(this: Arc<Self>) {
        this.pstate.store(ProcessState::Killed, Ordering::SeqCst);
        scheduler::add_to_schedule_queue(this);
    }
}

impl Drop for ProcessStruct {
    fn drop(&mut self) {
        // todo this should be a debug !
        info!("Dropped a process : {:?}", self)
    }
}
