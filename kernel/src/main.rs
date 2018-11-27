//! KFS
//!
//! A small kernel written in rust for shit and giggles. Also, hopefully the
//! last project I'll do before graduating from 42 >_>'.
//!
//! Currently doesn't do much, besides booting and printing Hello World on the
//! screen. But hey, that's a start.

#![feature(lang_items, start, asm, global_asm, compiler_builtins_lib, naked_functions, core_intrinsics, const_fn, abi_x86_interrupt, allocator_api, alloc, box_syntax, no_more_cas, const_vec_new, range_contains)]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![warn(unused)]
#![allow(unused_unsafe)]
#![allow(unreachable_code)]
#![allow(dead_code)]
#![recursion_limit = "1024"]

#[cfg(not(target_os = "none"))]
use std as core;

extern crate bit_field;
#[macro_use]
extern crate lazy_static;
extern crate multiboot2;
#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate static_assertions;
#[macro_use]
extern crate alloc;
extern crate linked_list_allocator;
#[macro_use]
extern crate log;
extern crate smallvec;
extern crate hashmap_core;
extern crate xmas_elf;
extern crate rustc_demangle;
extern crate byteorder;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate bitfield;
extern crate kfs_libkern;

use core::fmt::Write;
use alloc::prelude::*;

mod paging;
mod event;
mod error;
mod log_impl;
#[macro_use]
mod i386;
#[cfg(target_os = "none")]
mod gdt;
mod interrupts;
mod frame_allocator;

mod heap_allocator;
mod io;
mod devices;
mod sync;
mod process;
mod scheduler;
mod mem;
mod ipc;
mod elf_loader;
mod utils;
mod checks;

// Make rust happy about rust_oom being no_mangle...
pub use heap_allocator::rust_oom;

#[global_allocator]
static ALLOCATOR: heap_allocator::Allocator = heap_allocator::Allocator::new();

use i386::stack;
use paging::{PAGE_SIZE, MappingFlags};
use mem::VirtualAddress;
use process::{ProcessStruct, ThreadStruct};

unsafe fn force_double_fault() {
    loop {
        asm!("push 0" :::: "intel", "volatile");
    }
}

fn main() {
    info!("Loading all the init processes");
    for module in i386::multiboot::get_boot_information().module_tags().skip(1) {
        info!("Loading {}", module.name());
        let mapped_module = elf_loader::map_grub_module(module);
        let proc = ProcessStruct::new(String::from(module.name()), elf_loader::get_iopb(&mapped_module));
        let (ep, sp) = {
                let mut pmemlock = proc.pmemory.lock();

                let ep = elf_loader::load_builtin(&mut pmemlock, &mapped_module);

                let stack = pmemlock.find_available_space(5 * PAGE_SIZE)
                    .expect(&format!("Cannot create a stack for process {:?}", proc));
                pmemlock.guard(stack, PAGE_SIZE).unwrap();
                pmemlock.create_regular_mapping(stack + PAGE_SIZE, 4 * PAGE_SIZE, MappingFlags::u_rw()).unwrap();

                (VirtualAddress(ep), stack + 5 * PAGE_SIZE)
        };
        let thread = ThreadStruct::new(&proc, ep, sp)
            .expect("failed creating thread for service");
        ThreadStruct::start(thread)
            .expect("failed starting thread for service");
    }

    let lock = sync::SpinLockIRQ::new(());
    loop {
        // TODO: Exit process.
        let _ = scheduler::unschedule(&lock, lock.lock());
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub unsafe extern fn start() -> ! {
    asm!("
        // Memset the bss. Hopefully memset doesn't actually use the bss...
        mov eax, BSS_END
        sub eax, BSS_START
        push eax
        push 0
        push BSS_START
        call memset
        add esp, 12

        // Save multiboot infos addr present in ebx
        push ebx
        call common_start" : : : : "intel", "volatile");
    core::intrinsics::unreachable()
}

/// CRT0 starts here.
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn common_start(multiboot_info_addr: usize) -> ! {
    use devices::rs232::{SerialAttributes, SerialColor};

    log_impl::early_init();


    let log = &mut devices::rs232::SerialLogger;
    // Say hello to the world
    let _ = writeln!(log, "\n# Welcome to {}KFS{}!\n",
        SerialAttributes::fg(SerialColor::LightCyan),
        SerialAttributes::default());

    // Parse the multiboot infos
    let boot_info = unsafe { multiboot2::load(multiboot_info_addr) };
    info!("Parsed multiboot informations");

    // Setup frame allocator
    frame_allocator::init(&boot_info);
    info!("Initialized frame allocator");

    // Set up (read: inhibit) the GDT.
    info!("Initializing gdt...");
    gdt::init_gdt();
    info!("Gdt initialized");

    i386::multiboot::init(boot_info);

    log_impl::init();

    unsafe { devices::pit::init_channel_0() };
    info!("Initialized PIT");

    info!("Enabling interrupts");
    unsafe { interrupts::init(); }

    //info!("Disable timer interrupt");
    //devices::pic::get().mask(0);

    info!("Becoming the first process");
    unsafe { scheduler::create_first_process() };

    info!("Calling main()");

    main();
    // Die !
    // We shouldn't reach this...
    loop {
        #[cfg(target_os = "none")]
        unsafe { asm!("HLT"); }
    }
}

#[cfg(target_os = "none")]
#[lang = "eh_personality"] #[no_mangle] pub extern fn eh_personality() {}

/// The function executed on a panic! Can also be called at any moment.
/// Will print some useful debugging information, and never return.
fn do_panic(msg: core::fmt::Arguments, esp: usize, ebp: usize, eip: usize) -> ! {

    // Disable interrupts forever!
    unsafe { sync::permanently_disable_interrupts(); }
    // Don't deadlock in the logger
    unsafe { SerialLogger.force_unlock(); }

    //todo: force unlock the KernelMemory lock
    //      and also the process memory lock for userspace stack dumping (only if panic-on-excetpion ?).

    use devices::rs232::SerialLogger;

    let _ = writeln!(SerialLogger, "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\
                                    ! Panic! at the disco\n\
                                    ! {}\n\
                                    !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
                     msg);

    // Parse the ELF to get the symbol table.
    // We must not fail, so this means a lot of Option checking :/
    use xmas_elf::symbol_table::Entry32;
    use xmas_elf::sections::SectionData;
    use xmas_elf::ElfFile;
    use elf_loader::MappedGrubModule;

    let mapped_kernel_elf = i386::multiboot::try_get_boot_information()
        .and_then(|info| info.module_tags().nth(0))
        .and_then(|module| Some(elf_loader::map_grub_module(module)));

    /// Gets the symbol table of a mapped module.
    fn get_symbols<'a>(mapped_kernel_elf: &'a Option<MappedGrubModule>) -> Option<(&'a ElfFile<'a>, &'a[Entry32])> {
        let module = mapped_kernel_elf.as_ref()?;
        let elf = module.elf.as_ref().ok()?;
        let data = elf.find_section_by_name(".symtab")?
            .get_data(elf).ok()?;
        let st = match data {
            SectionData::SymbolTable32(st) => st,
            _ => return None
        };
        Some((elf, st))
    }

    let elf_and_st = get_symbols(&mapped_kernel_elf);

    if elf_and_st.is_none() {
        let _ = writeln!(SerialLogger, "Panic handler: Failed to get kernel elf symbols");
    }

    // Then print the stack
    stack::KernelStack::dump_stack(esp, ebp, eip, elf_and_st);

    let _ = writeln!(SerialLogger, "Thread : {:#x?}", scheduler::try_get_current_thread());

    let _ = writeln!(SerialLogger, "!!!!!!!!!!!!!!!END PANIC!!!!!!!!!!!!!!");

    loop { unsafe { asm!("HLT"); } }
}

#[cfg(target_os = "none")]
#[panic_handler] #[no_mangle]
pub extern fn panic_fmt(p: &::core::panic::PanicInfo) -> ! {
    // call do_panic() with our current esp, ebp, and eip.
    do_panic(format_args!("{}", p), esp!(), ebp!(), i386::registers::eip());
}
