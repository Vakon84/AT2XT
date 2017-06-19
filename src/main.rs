#![no_std]
#![no_main]
#![feature(asm)]
#![feature(used)]
#![feature(lang_items)]
#![feature(global_asm)]
#![feature(abi_msp430_interrupt)]
#![feature(const_fn)]

extern crate volatile_register;
use volatile_register::RW;
use volatile_register::RO;

mod keymap;
use keymap::to_xt;

mod keybuffer;
use keybuffer::{KeycodeBuffer, KeyIn};

mod driver;
use driver::KeyboardPins;

mod interrupt;
use interrupt::*;

mod util;


global_asm!(r#"
    .globl reset_handler
reset_handler:
    mov #__stack, r1
    br #main
"#);

#[used]
#[link_section = "__interrupt_vector_reset"]
static RESET_VECTOR: unsafe extern "msp430-interrupt" fn() = reset_handler;

extern "msp430-interrupt" {
    fn reset_handler();
}

#[used]
#[link_section = "__interrupt_vector_timer0_a0"]
static TIM0_VECTOR: unsafe extern "msp430-interrupt" fn() = timer0_handler;

unsafe extern "msp430-interrupt" fn timer0_handler() {
    // you can do something here
}

#[used]
#[link_section = "__interrupt_vector_port1"]
static PORTA_VECTOR: unsafe extern "msp430-interrupt" fn() = porta_handler;

unsafe extern "msp430-interrupt" fn porta_handler() {
    // if acting_as_host() {

    // } else {
    let full : bool;

    // Are the buffer functions safe in nested interrupts? Is it possible to use tokens/manual
    // sync for nested interrupts while not giving up safety?
    // Example: Counter for nest level when updating buffers. If it's ever more than one, panic.
    unsafe {
        KEY_IN.shift_in(KEYBOARD_PINS.at_data.is_set());
        full = KEY_IN.is_full();
    }

    if full {
        KEYBOARD_PINS.at_inhibit(); // Ask keyboard to not send anything while processing keycode.

        unsafe {
            let mut key : u16 = match KEY_IN.take() {
                Some(k) => k,
                None => 0, // unreachable
            }; // unwrap doesn't work here... rust_begin_unwind can't be removed.

            IN_BUFFER.put(key);
            KEY_IN.clear();
        }

        KEYBOARD_PINS.at_idle();
    }
    // }

    KEYBOARD_PINS.clear_at_clk_int();
}

extern "C" {
    static mut WDTCTL: RW<u16>;
    static mut BCSCTL1: RW<u8>;
    static mut BCSCTL2: RW<u8>;
    // TACCR0
    // TACTL
    // TACCTL0
}

static mut IN_BUFFER : KeycodeBuffer = KeycodeBuffer::new();
static mut KEY_IN : KeyIn = KeyIn::new();
static KEYBOARD_PINS : KeyboardPins = KeyboardPins::new();

#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe {
        WDTCTL.write(0x5A00 + 0x80); // WDTPW + WDTHOLD
    }

    KEYBOARD_PINS.idle(); // FIXME: Can we make this part of new()?

    unsafe {
        BCSCTL1.write(0x88); // XT2 off, Range Select 7.
        BCSCTL2.write(0x04); // Divide submain clock by 4.
    }


    'get_command: loop {
        // P1OUT.modify(|x| !x);
        // delay(40000);

        // Run state machine/send reply. Receive new cmd.

        // The micro spends the majority of its life idle. It is possible for the host PC and
        // the keyboard to send data to the micro at the same time. To keep control flow simple,
        // the micro will only respond to host PC acknowledge requests if its idle.

        unsafe {
            'idle: while IN_BUFFER.is_empty() {

                // If host computer wants to reset
                if KEYBOARD_PINS.xt_sense.is_unset() {
                    send_byte_to_pc(0xAA);
                    continue 'get_command;
                }
            }

            send_byte_to_pc(to_xt(IN_BUFFER.take().unwrap() as u8));
        }
    }
}

pub fn send_xt_bit(bit : u8) -> () {
    if bit == 1 {
        KEYBOARD_PINS.xt_data.set();
    } else {
        KEYBOARD_PINS.xt_data.unset();
    }

    KEYBOARD_PINS.xt_clk.unset();
    unsafe { delay(88); } // 55 microseconds at 1.6 MHz
    // PAUSE
    KEYBOARD_PINS.xt_clk.set();
}

pub fn send_byte_to_pc(mut byte : u8) -> () {
    // The host cannot send data; the only communication it can do with the micro is pull
    // the CLK (reset) and DATA (shift register full) low.
    // Wait for the host to release the lines.
    while KEYBOARD_PINS.xt_clk.is_unset() || KEYBOARD_PINS.xt_data.is_unset() {

    }

    KEYBOARD_PINS.xt_out();
    send_xt_bit(0);
    send_xt_bit(1);

    for _ in 0..8 {
        send_xt_bit((byte & 0x01)); /* Send data... */
		byte = byte >> 1;
    }

    KEYBOARD_PINS.xt_in();
}





unsafe fn delay(n: u16) {
    asm!(r#"
1:
    dec $0
    jne 1b
    "# :: "{r12}"(n) : "r12" : "volatile");
}

#[used]
#[lang = "panic_fmt"]
extern "C" fn panic_fmt() -> ! {
    loop {}
}
