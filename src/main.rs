//! KFS
//!
//! A small kernel written in rust for shit and giggles. Also, hopefully the
//! last project I'll do before graduating from 42 >_>'.
//!
//! Currently doesn't do much, besides booting and printing Hello World on the
//! screen. But hey, that's a start.

#![feature(lang_items, start, asm, global_asm, compiler_builtins_lib, naked_functions, core_intrinsics, const_fn, abi_x86_interrupt, iterator_step_by, used, allocator_api, alloc, panic_implementation)]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(unused)]
#[cfg(not(target_os = "none"))]
use std as core;

extern crate arrayvec;
extern crate ascii;
extern crate bit_field;
#[macro_use]
extern crate lazy_static;
extern crate spin;
extern crate multiboot2;
#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate static_assertions;
extern crate alloc;
extern crate linked_list_allocator;
extern crate gif;
#[macro_use]
extern crate log;
extern crate smallvec;

use ascii::AsAsciiStr;
use core::fmt::Write;
use alloc::*;

mod logger;
mod log_impl;
pub use logger::*;
pub use devices::vgatext::VGATextLogger;
pub use devices::rs232::SerialLogger;
use frame_alloc::{PhysicalAddress, Frame};
use paging::KernelLand;

mod i386;
#[cfg(target_os = "none")]
mod gdt;
mod utils;
mod frame_alloc;
mod heap_allocator;
mod io;
mod devices;

#[global_allocator]
static ALLOCATOR: heap_allocator::Allocator = heap_allocator::Allocator::new();

pub use frame_alloc::FrameAllocator;
pub use i386::paging;
pub use i386::stack;
use i386::paging::{InactivePageTables, PageTablesSet, EntryFlags};

fn main() {
    let loggers = &mut Loggers;
    loggers.println("Hello world!      ");
    loggers.println_attr("Whoah, nice color",
                      LogAttributes::new_fg_bg(LogColor::Pink, LogColor::Cyan));
    loggers.println_attr("such hues",
                          LogAttributes::new_fg_bg(LogColor::Magenta, LogColor::LightGreen));
    loggers.println_attr("very polychromatic",
                           LogAttributes::new_fg_bg(LogColor::Yellow, LogColor::Pink));

    let mymem = FrameAllocator::alloc_frame();
    info!("Allocated frame {:x?}", mymem);
    FrameAllocator::free_frame(mymem);
    info!("Freed frame {:x?}", mymem);

    writeln!(Loggers, "----------");

    let page1 = ::paging::get_page::<::paging::UserLand>();
    info!("Got page {:#x}", page1.addr());
    let page2 = ::paging::get_page::<::paging::UserLand>();
    info!("Got page {:#x}", page2.addr());

    info!("----------");

    let mut inactive_pages = InactivePageTables::new();
    info!("Created new tables");
    let page_innactive = inactive_pages.get_page::<paging::UserLand>();
    info!("Mapped inactive page {:#x}", page_innactive.addr());
    unsafe { inactive_pages.switch_to() };
    info!("Switched to new tables");
    let page_active = ::paging::get_page::<::paging::UserLand>();
    info!("Got page {:#x}", page_active.addr());

    info!("Testing some string heap alloc: {}", String::from("Hello World"));

    // Let's GIF.
    let mut vbe = unsafe {
        devices::vbe::Framebuffer::new(i386::multiboot::get_boot_information())
    };
    let mut reader = gif::Decoder::new(&LOUIS[..]).read_info().unwrap();
    let mut buf = Vec::new();
    loop {
        {
            let end = reader.next_frame_info().unwrap().is_none();
            if end {
                reader = gif::Decoder::new(&LOUIS[..]).read_info().unwrap();
                let _ = reader.next_frame_info().unwrap().unwrap();
            }
        }
        buf.resize(reader.buffer_size(), 0);
        info!("Buf: {:#010x}, len: {}", buf.as_ptr() as usize, buf.len());
        // simulate read into buffer
        reader.read_into_buffer(&mut buf[..]);
        for y in 0..(reader.height() as usize) {
            for x in 0..(reader.width() as usize) {
                let frame_coord = (y * reader.width() as usize + x) * 4;
                let vbe_coord = (y * vbe.width() + x) * 4;
                vbe.get_fb()[vbe_coord] = buf[frame_coord + 2];
                vbe.get_fb()[vbe_coord + 1] = buf[frame_coord + 1];
                vbe.get_fb()[vbe_coord + 2] = buf[frame_coord];
                vbe.get_fb()[vbe_coord + 3] = 0xFF;
            }
        }
    }
}

static LOUIS: &'static [u8; 1318100] = include_bytes!("../img/meme3.gif");

#[repr(align(4096))]
pub struct AlignedStack([u8; 4096 * 4]);

#[link_section = ".stack"]
pub static mut STACK: AlignedStack = AlignedStack([0; 4096 * 4]);

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

        // Create the stack
        mov esp, $0
        add esp, 16383
        mov ebp, esp
        // Save multiboot infos addr present in ebx
        push ebx
        call common_start" : : "m"(&STACK) : : "intel", "volatile");
    core::intrinsics::unreachable()
}

/// CRT0 starts here.
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn common_start(multiboot_info_addr: usize) -> ! {
    log_impl::early_init();

    // Do whatever is necessary to have a proper environment here.

    // Register some loggers
    static mut VGATEXT: VGATextLogger = VGATextLogger;
    Loggers::register_logger("VGA text mode", unsafe { &mut VGATEXT });
    static mut SERIAL: SerialLogger = SerialLogger;
    Loggers::register_logger("Serial", unsafe { &mut SERIAL });

    info!("Clearing screen...");
    let vga_screen = &mut VGATextLogger;
    vga_screen.clear();

    let loggers = &mut Loggers;
    // Say hello to the world
    write!(Loggers, "\n# Welcome to ");
    loggers.print_attr("KFS",
                             LogAttributes::new_fg(LogColor::LightCyan));
    writeln!(Loggers, "!\n");

    // Set up (read: inhibit) the GDT.
    gdt::init_gdt();
    info!("Gdt initialized");

    // Parse the multiboot infos
    let boot_info = unsafe { multiboot2::load(multiboot_info_addr) };
    info!("Parsed multiboot informations");

    // Setup frame allocator
    FrameAllocator::init(&boot_info);
    info!("Initialized frame allocator");

    // Move the multiboot_header to a single page. Because GRUB sucks. Seriously.
    let multiboot_info_frame = FrameAllocator::alloc_frame();
    let total_size = unsafe {
        // Safety: multiboot_info_addr should always be valid, provided the
        // bootloader ಠ_ಠ
        *(multiboot_info_addr as *const u32) as usize
    };
    assert!(total_size <= paging::PAGE_SIZE, "Expected multiboot info to fit in a frame");
    unsafe {
        // Safety: We just allocated this frame. What could go wrong?
        core::ptr::copy(multiboot_info_addr as *const u8, multiboot_info_frame.address().addr() as *mut u8, total_size);
    }
    // TODO: Free the god damned multiboot frames.

    // Create page tables with the right access rights for each kernel section
    let mut page_tables =
    unsafe { paging::map_kernel(&boot_info) };
    info!("Mapped the kernel");

    // Map the boot_info.
    let multiboot_info_vaddr = page_tables.map_frame::<KernelLand>(multiboot_info_frame, EntryFlags::PRESENT);

    // Start using these page tables
    unsafe { page_tables.enable_paging() }
    info!("= Paging on");

    unsafe {
        i386::multiboot::init(multiboot2::load(multiboot_info_vaddr.addr()));
    }

    log_impl::init();

    let new_stack = stack::KernelStack::allocate_stack()
        .expect("Failed to allocate new kernel stack");
    unsafe { new_stack.switch_to(common_start_continue_stack) }
    unreachable!()
}

/// When we switch to a new valid kernel stack during init, we can't return now that the stack is empty
/// so we need to call some function that will proceed with the end of the init procedure
/// This is some function
#[cfg(target_os = "none")]
#[no_mangle]
pub fn common_start_continue_stack() -> ! {
    info!("Switched to new kernel stack");

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

#[cfg(target_os = "none")]
#[panic_implementation] #[no_mangle]
pub extern fn panic_fmt(p: &::core::panic::PanicInfo) -> ! {

    unsafe { Loggers.force_unlock(); }
    let _ = writeln!(Loggers, "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\
                               ! Panic! at the disco\n\
                               ! {}\n\
                               !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
                     p);

    loop { unsafe { asm!("HLT"); } }
}

macro_rules! multiboot_header {
    //($($expr:tt)*) => {
    ($($name:ident: $tagty:ident :: $method:ident($($args:expr),*)),*) => {
        #[repr(C)]
        #[allow(dead_code)]
        pub struct MultiBootHeader {
            magic: u32,
            architecture: u32,
            header_length: u32,
            checksum: u32,
            $($name: $tagty),*
        }

        #[used]
        #[cfg_attr(target_os = "none", link_section = ".multiboot_header")]
        pub static MULTIBOOT_HEADER: MultiBootHeader = MultiBootHeader {
            magic: 0xe85250d6,
            architecture: 0,
            header_length: core::mem::size_of::<MultiBootHeader>() as u32,
            checksum: u32::max_value() - (0xe85250d6 + 0 + core::mem::size_of::<MultiBootHeader>() as u32) + 1,
            $($name: $tagty::$method($($args),*)),*
        };
    }
}

#[repr(C, align(8))]
struct EndTag {
    tag: u16,
    flag: u16,
    size: u32
}

impl EndTag {
    const fn default() -> EndTag {
        EndTag {
            tag: 0,
            flag: 0,
            size: ::core::mem::size_of::<Self>() as u32
        }
    }
}

#[repr(C, align(8))]
struct FramebufferTag {
    tag: u16,
    flags: u16,
    size: u32,
    width: u32,
    height: u32,
    depth: u32
}

impl FramebufferTag {
    const fn new(width: u32, height: u32, depth: u32) -> FramebufferTag {
        FramebufferTag {
            tag: 5,
            flags: 0,
            size: ::core::mem::size_of::<Self>() as u32,
            width: width,
            height: height,
            depth: depth
        }
    }
}

multiboot_header! {
    framebuffer: FramebufferTag::new(1280, 800, 32),
    end: EndTag::default()
}
