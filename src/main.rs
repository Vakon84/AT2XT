#![no_main]
#![no_std]
#![feature(abi_msp430_interrupt)]
#![deny(unsafe_code)]

extern crate panic_msp430;

use bit_reverse::BitwiseReverse;
use core::cell::{Cell, RefCell};
use msp430::{critical_section as mspcs, interrupt::CriticalSection, interrupt::Mutex};
use msp430_rt::entry;
use msp430g2211::{interrupt, Peripherals};
use portable_atomic::{AtomicBool, Ordering};

mod keyfsm;
use keyfsm::{Cmd, Fsm, LedMask, ProcReply};

mod keybuffer;
use keybuffer::{KeyIn, KeyOut, KeycodeBuffer};

mod driver;
use driver::Pins;

mod peripheral;
use peripheral::At2XtPeripherals;

macro_rules! delay_us {
    ($u:expr) => {
        // Timer is 100000 Hz, thus granularity of 10us.
        delay(($u / 10) + 1)
    };
}

static TIMEOUT: AtomicBool = AtomicBool::new(false);
static HOST_MODE: AtomicBool = AtomicBool::new(false);
static DEVICE_ACK: AtomicBool = AtomicBool::new(false);

static IN_BUFFER: Mutex<RefCell<KeycodeBuffer>> = Mutex::new(RefCell::new(KeycodeBuffer::new()));
static KEY_IN: Mutex<Cell<KeyIn>> = Mutex::new(Cell::new(KeyIn::new()));
static KEY_OUT: Mutex<Cell<KeyOut>> = Mutex::new(Cell::new(KeyOut::new()));

#[interrupt]
fn TIMERA0(cs: CriticalSection) {
    TIMEOUT.store(true, Ordering::SeqCst);

    // Use unwrap b/c within interrupt handlers, if we can't get access to
    // peripherals right away, there's no point in continuing.
    let timer: &msp430g2211::TIMER_A2 = At2XtPeripherals::periph_ref(cs).unwrap();
    // Writing 0x0000 stops Timer in MC1.
    timer.taccr0.write(|w| w.taccr0().bits(0x0000));
    // CCIFG will be reset when entering interrupt; no need to clear it.
    // Nesting is disabled, and chances of receiving second CCIFG in the ISR
    // are nonexistant.
}

#[interrupt]
fn PORT1(cs: CriticalSection) {
    let port = At2XtPeripherals::periph_ref(cs).unwrap();

    if HOST_MODE.load(Ordering::SeqCst) {
        let mut keyout = KEY_OUT.borrow(cs).get();

        if let Some(k) = keyout.shift_out() {
            if k {
                driver::set(port, Pins::AT_DATA);
            } else {
                driver::unset(port, Pins::AT_DATA);
            }

            // Immediately after sending out the Stop Bit, we should release the lines.
            if keyout.is_empty() {
                driver::at_idle(port);
            }
        } else {
            // TODO: Is it possible to get a spurious clock interrupt and
            // thus skip this logic?
            if driver::is_unset(port, Pins::AT_DATA) {
                DEVICE_ACK.store(true, Ordering::SeqCst);
                keyout.clear();
            }
        }

        KEY_OUT.borrow(cs).set(keyout);
    } else {
        let mut keyin = KEY_IN.borrow(cs).get();

        // Are the buffer functions safe in nested interrupts? Is it possible to use tokens/manual
        // sync for nested interrupts while not giving up safety?
        // Example: Counter for nest level when updating buffers. If it's ever more than one, panic.
        if keyin.shift_in(driver::is_set(port, Pins::AT_DATA)).is_err() {
            driver::at_inhibit(port); // Ask keyboard to not send anything while processing keycode.

            if let Some(k) = keyin.take() {
                if let Ok(mut b) = IN_BUFFER.borrow(cs).try_borrow_mut() {
                    // Dropping keys when the buffer is full is in line
                    // with what AT/XT hosts do. Saves 2 bytes on panic :)!
                    #[allow(clippy::let_underscore_must_use)]
                    {
                        let _ = b.put(k);
                    }
                }
            }

            keyin.clear();

            driver::at_idle(port);
        }

        KEY_IN.borrow(cs).set(keyin);
    }

    driver::clear_at_clk_int(port);
}

fn init(cs: CriticalSection) {
    let p = Peripherals::take().unwrap();

    p.WATCHDOG_TIMER
        .wdtctl
        .write(|w| w.wdtpw().password().wdthold().set_bit());

    driver::idle(&p.PORT_1_2);

    let calcb1 = p.CALIBRATION_DATA.calbc1_1mhz.read().calbc1_1mhz().bits();
    let caldco = p.CALIBRATION_DATA.calbc1_1mhz.read().calbc1_1mhz().bits();

    // We want a nominally 1.6MHz clock (to get an easily-divisible timer of
    // 100kHz). Higher frequencies are fine, but even a bit lower than 1.6MHz
    // runs into timing problems servicing interrupts IME.
    //
    // According to the MSP430G2211 datasheet:
    // * Every increment of the bottom 4 bits of BCSCTL1 (RSEL) increments the
    //   clock frequency by 1.35.
    // * Every increment of the top 3 bits of DCOCTL (DSO) increments the clock
    //   frequency by 1.08.
    // * The bottom 5 bits of DCOCTL (MOD) fine-tunes the clock frequency
    //   between frequency F and frequency F * 1.08 (except for DSO == 7, in
    //   which case MOD has no effect).
    //
    // For this application, we leave MOD alone, assume RSEL is < 14 (safe for
    // properly calibrated chips), and boost the freq from the calibrated 1MHz
    // value by 1.35^2*1.08. This is closer to 1.70MHz; we add some breathing
    // room because the 1MHz calibration value can vary up to 3% according to
    // the MSP430G2211 datasheet.
    p.SYSTEM_CLOCK
        .bcsctl1
        .write(|w| w.bcsctl1().bits(calcb1 + 2)); // XT2 off, Multiply freq by 1.35^2.
        // Assumes bottom 4 bits < 14, will spill into DIVA bits if violated.
    p.SYSTEM_CLOCK.dcoctl.write(|w| {
        w.dcoctl().bits(if caldco >= 32 {
            caldco - 32 // Divide by 1.08 if DCO bits nonzero.
        } else {
            caldco // Otherwise leave alone.
        })
    });
    p.SYSTEM_CLOCK.bcsctl2.write(|w| w.divs().divs_2()); // Divide submain clock by 4, nominally 400kHz.

    p.TIMER_A2.taccr0.write(|w| w.taccr0().bits(0x0000));
    p.TIMER_A2
        .tactl
        .write(|w| w.tassel().tassel_2().id().id_2().mc().mc_1()); // Divide by 4, use submain clock (100kHz).
    p.TIMER_A2.tacctl0.write(|w| w.ccie().set_bit());

    let shared = At2XtPeripherals {
        port: p.PORT_1_2,
        timer: p.TIMER_A2,
    };

    At2XtPeripherals::init(shared, cs).unwrap();
}

#[entry(interrupt_enable(pre_interrupt = init))]
fn main() -> ! {
    send_byte_to_at_keyboard(Cmd::RESET).unwrap();

    let mut loop_cmd: Cmd;
    let mut loop_reply: ProcReply = ProcReply::init();
    let mut fsm_driver: Fsm = Fsm::start();

    loop {
        // Run state machine/send reply. Receive new cmd.
        loop_cmd = fsm_driver.run(&loop_reply).unwrap();

        loop_reply = match loop_cmd {
            Cmd::ClearBuffer => {
                mspcs::with(|cs| {
                    // XXX: IN_BUFFER.borrow(cs).borrow_mut() and
                    // IN_BUFFER.borrow(cs).try_borrow_mut().unwrap()
                    // bring in dead formatting code! Use explicit
                    // if-let for now and handle errors by doing nothing.

                    if let Ok(mut b) = IN_BUFFER.borrow(cs).try_borrow_mut() {
                        b.flush()
                    }
                });
                ProcReply::ClearedBuffer
            }
            Cmd::ToggleLed(m) => {
                toggle_leds(m).unwrap();
                ProcReply::LedToggled(m)
            }
            Cmd::SendXtKey(k) => {
                send_byte_to_pc(k).unwrap();
                ProcReply::SentKey(k)
            }
            Cmd::WaitForKey => {
                // The micro spends the majority of its life idle. It is possible for the host PC and
                // the keyboard to send data to the micro at the same time. To keep control flow simple,
                // the micro will only respond to host PC acknowledge requests if its idle.
                fn reset_requested() -> bool {
                    mspcs::with(|cs| {
                        let port = At2XtPeripherals::periph_ref(cs).unwrap();

                        driver::is_unset(port, Pins::XT_SENSE)
                    })
                }

                fn attempt_take() -> Option<u16> {
                    mspcs::with(|cs| {
                        IN_BUFFER
                            .borrow(cs)
                            .try_borrow_mut()
                            // Staying in idle state and busy-waiting is reasonable behavior for
                            // now if we couldn't borrow the IN_BUFFER.
                            .map_or(None, |mut b| b.take())
                    })
                }

                loop {
                    if let Some(b_in) = attempt_take() {
                        let mut bits_in = b_in;
                        bits_in &= !(0x4000 + 0x0001); // Mask out start/stop bit.
                        bits_in >>= 2; // Remove stop bit and parity bit (FIXME: Check parity).
                        break ProcReply::GrabbedKey((bits_in as u8).swap_bits());
                    }
                    // If host computer wants to reset
                    if reset_requested() {
                        send_byte_to_at_keyboard(Cmd::RESET).unwrap();
                        send_byte_to_pc(Cmd::SELF_TEST_PASSED).unwrap();
                        break ProcReply::KeyboardReset;
                    }
                }
            }
        }
    }
}

pub fn send_xt_bit(bit: u8) -> Result<(), ()> {
    mspcs::with(|cs| {
        let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        if bit == 1 {
            driver::set(port, Pins::XT_DATA);
        } else {
            driver::unset(port, Pins::XT_DATA);
        }

        driver::unset(port, Pins::XT_CLK);

        Ok(())
    })?;

    delay_us!(55)?;

    mspcs::with(|cs| {
        let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        driver::set(port, Pins::XT_CLK);
        Ok(())
    })?;

    Ok(())
}

pub fn send_byte_to_pc(mut byte: u8) -> Result<(), ()> {
    fn wait_for_host() -> Result<bool, ()> {
        mspcs::with(|cs| {
            let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

            let clk_or_data_unset =
                driver::is_unset(port, Pins::XT_CLK) || driver::is_unset(port, Pins::XT_DATA);

            if !clk_or_data_unset {
                driver::xt_out(port);
            }

            Ok(clk_or_data_unset)
        })
    }

    // The host cannot send data; the only communication it can do with the micro is pull
    // the CLK (reset) and DATA (shift register full) low.
    // Wait for the host to release the lines.
    while wait_for_host()? {}

    send_xt_bit(0)?;
    send_xt_bit(1)?;

    for _ in 0..8 {
        send_xt_bit(byte & 0x01)?; /* Send data... */
        byte >>= 1;
    }

    mspcs::with(|cs| {
        let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        driver::xt_in(port);
        Ok(())
    })?;

    Ok(())
}

fn send_byte_to_at_keyboard(byte: u8) -> Result<(), ()> {
    // TODO: What does the AT keyboard protocol say about retrying xfers
    // when inhibiting communication? Does the keyboard retry from the beginning
    // or from the interrupted bit? Right now, we don't flush KeyIn, so
    // we do it from the interrupted bit. This seems to work fine.
    fn wait_for_at_keyboard() -> Result<bool, ()> {
        mspcs::with(|cs| {
            let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

            let unset = driver::is_unset(port, Pins::AT_CLK);

            if !unset {
                driver::at_inhibit(port);
            }

            Ok(unset)
        })
    }

    mspcs::with(|cs| {
        let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        let mut key_out = KEY_OUT.borrow(cs).get();

        key_out.put(byte)?;

        // Safe outside of critical section: As long as HOST_MODE is
        // not set, it's not possible for the interrupt
        // context to touch this variable.
        KEY_OUT.borrow(cs).set(key_out);
        driver::disable_at_clk_int(port);
        Ok(())
    })?;

    /* If/when timer int is enabled, this loop really needs to allow preemption during
    I/O read. Can it be done without overhead of CriticalSection? */
    while wait_for_at_keyboard()? {}

    delay_us!(100)?;

    mspcs::with(|cs| {
        let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        driver::unset(port, Pins::AT_DATA);
        Ok(())
    })?;

    delay_us!(33)?;

    mspcs::with(|cs| {
        let port = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        driver::set(port, Pins::AT_CLK);
        driver::mk_in(port, Pins::AT_CLK);
        driver::clear_at_clk_int(port);

        driver::enable_at_clk_int(port);
        HOST_MODE.store(true, Ordering::SeqCst);
        DEVICE_ACK.store(false, Ordering::SeqCst);
        Ok(())
    })?;

    while !DEVICE_ACK.load(Ordering::SeqCst) {}

    HOST_MODE.store(false, Ordering::SeqCst);

    Ok(())
}

fn toggle_leds(mask: LedMask) -> Result<(), ()> {
    send_byte_to_at_keyboard(Cmd::SET_LEDS)?;
    delay_us!(3000)?;
    send_byte_to_at_keyboard(mask.bits())?;
    Ok(())
}

fn delay(time: u16) -> Result<(), ()> {
    start_timer(time)?;
    while !TIMEOUT.load(Ordering::SeqCst) {}

    Ok(())
}

fn start_timer(time: u16) -> Result<(), ()> {
    mspcs::with(|cs| {
        let timer: &msp430g2211::TIMER_A2 = At2XtPeripherals::periph_ref(cs).ok_or(())?;

        TIMEOUT.store(false, Ordering::SeqCst);
        timer.taccr0.write(|w| w.taccr0().bits(time));
        Ok(())
    })
}
