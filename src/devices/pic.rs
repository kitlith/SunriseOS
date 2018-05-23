//! Driver for the 8259 Programmable Interrupt Controller.
//!
//! Only handles the usual case of two PICs in a cascading setup, where the
//! SLAVE is setup to cascade to the line 2 of the MASTER.

use i386::instructions::port::*;

bitflags! {
    /// The first control word sent to the PIC.
    struct ICW1: u8 {
        /// If this bit is set, ICW4 has to be read. If ICW4 is not needed, set
        /// ICW4 to 0
        const ICW4      = 0x01;
        /// Single. Means that this is the only 8259A in the system. If SINGLE
        // is 1, no ICW3 will be issued.
        const SINGLE    = 0x02;
        /// Call Address Interval. Used only in 8085, not 8086. 1=ISR's are 4
        /// bytes apart (0200, 0204, etc) 0=ISR's are 8 byte apart (0200, 0208,
        /// etc)
        const INTERVAL4 = 0x04;
        /// If LEVEL = 1, then the 8259A will operate in the level interrupt
        /// mode. Edge detect logic on the interrupt inputs will be disabled.
        const LEVEL     = 0x08;
        /// Should always be set to 1.
        const INIT      = 0x10;
    }
}
const ICW4_8086: u8     = 0x01;       /* 8086/88 (MCS-80/85) mode */
//const icw4_auto         = 0x02;       /* Auto (normal) EOI */
//const icw4_buf_slave    = 0x08;       /* Buffered mode/slave */
//const icw4_buf_master   = 0x0C;       /* Buffered mode/master */
//const icw4_sfnm         = 0x10;       /* Special fully nested (not) */

pub static mut MASTER: Pic = unsafe { Pic::new(0x20) };
pub static mut SLAVE: Pic = unsafe { Pic::new(0xA0) };

/// A single PIC8259 device.
pub struct Pic {
    port_cmd: u16,
    port_data: u16
}

fn io_wait() {
    // Port 0x80 is used for 'checkpoints' during POST.
    // The Linux kernel seems to think it is free for use :-/
    unsafe { outb(0x80, 0); }
}

/// setup the 8259 pic. redirect the IRQ to user interrupt 32+.
pub unsafe fn init() {
    // save masks
    let a1 = inb(MASTER.port_data);
    let a2 = inb(SLAVE.port_data);

    // starts the initialization sequence (in cascade mode)
    outb(MASTER.port_cmd, (ICW1::INIT | ICW1::ICW4).bits());
    io_wait();
    outb(SLAVE.port_cmd, (ICW1::INIT | ICW1::ICW4).bits());
    io_wait();
    // ICW2: Master PIC vector offset
    outb(MASTER.port_data, 0x20);
    io_wait();
    // ICW2: Slave PIC vector offset
    outb(SLAVE.port_data, 0x28);
    io_wait();
    // ICW3: tell Master PIC that there is a slave PIC at IRQ2 (0000 0100)
    outb(MASTER.port_data, 4);
    io_wait();
    // ICW3: tell Slave PIC its cascade identity (0000 0010)
    outb(SLAVE.port_data, 2);
    io_wait();

    outb(MASTER.port_data, ICW4_8086);
    io_wait();
    outb(SLAVE.port_data, ICW4_8086);
    io_wait();

    outb(MASTER.port_data, a1);   // restore saved masks.
    outb(SLAVE.port_data, a2);
}

impl Pic {
    /// Creates a new Pic device.
    ///
    /// # Safety
    ///
    /// The port should map to a proper PIC device. Sending invalid data to a
    /// random device can lead to memory unsafety.
    const unsafe fn new(port_base: u16) -> Pic {
        Pic {
            port_cmd: port_base,
            port_data: port_base + 1
        }
    }

    /// Acknowledges an IRQ, allowing the PIC to send a new IRQ on the next
    /// cycle.
    pub fn acknowledge(&mut self) {
        unsafe {
            outb(self.port_cmd, 0x20);
        }
    }
}
