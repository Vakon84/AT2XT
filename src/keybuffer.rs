use interrupt::CriticalSectionToken;
use util;

pub struct KeycodeBuffer {
    head : u8,
    tail : u8,
    contents : [u16; 16],
}

impl KeycodeBuffer {
    pub const fn new() -> KeycodeBuffer {
        KeycodeBuffer {
            head : 0,
            tail : 0,
            contents : [0; 16],
        }
    }

    pub fn flush(&mut self, ctx : &CriticalSectionToken) -> () {
        let _ = ctx;
        self.tail = 0;
        self.head = 0;
    }

    pub fn is_empty(&self, ctx : &CriticalSectionToken) -> bool {
        let _ = ctx;
        (self.head - self.tail == 0)
    }

    pub fn put(&mut self, in_key : u16, ctx : &CriticalSectionToken) -> () {
        let _ = ctx;
        // TODO: A full buffer is an abnormal condition worth a panic/reset.

        self.contents[self.tail as usize] = in_key;
        self.tail = (self.tail + 1) % 16;
    }

    pub fn take(&mut self, ctx : &CriticalSectionToken) -> Option<u16> {
        if self.is_empty(ctx) {
            None
        } else {
            let out_key : u16 = self.contents[self.head as usize];
            self.head = (self.head + 1) % 16;
            Some(out_key)
        }
    }
}


pub struct KeyIn {
    pos : u8,
    contents : u16,
}

impl KeyIn {
    pub const fn new() -> KeyIn {
        KeyIn {
            pos : 0,
            contents : 0,
        }
    }

    pub fn is_full(&self) -> bool {
        self.pos >= 11
    }

    pub fn clear(&mut self, ctx : &CriticalSectionToken) {
        let _ = ctx;
        self.pos = 0;
        self.contents = 0;
    }

    pub fn shift_in(&mut self, bit : bool, ctx : &CriticalSectionToken) -> () {
        let _ = ctx;
        // TODO: A nonzero start value (when self.pos == 0) is a runtime invariant violation.
        let cast_bit : u16 = if bit {
                1
            } else {
                0
            };
        self.contents = (self.contents << 1) | cast_bit;
        self.pos = self.pos + 1;
    }

    pub fn take(&mut self, ctx : &CriticalSectionToken) -> Option<u16> {
        let _ = ctx;
        if !self.is_full() {
            None
        } else {
            self.pos = 0;
            Some(self.contents)
        }
    }
}


pub struct KeyOut {
    pos : u8,
    contents : u16,
}

impl KeyOut {
    pub const fn new() -> KeyOut {
        KeyOut {
            pos : 10,
            contents : 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pos > 9 // Data 0-7, Parity, and Stop. Start bit has to be handled specially b/c
                     // it's part of keyboard negotiation.
    }

    pub fn clear(&mut self, ctx : &CriticalSectionToken) {
        let _ = ctx;
        self.pos = 10;
        self.contents = 0;
    }

    pub fn shift_out(&mut self, ctx : &CriticalSectionToken) -> bool {
        let _ = ctx;
        // TODO: A nonzero start value (when self.pos == 0) is a runtime invariant violation.
        let cast_bit : bool = (self.contents & 0x01) == 1;
        self.contents = self.contents >> 1;
        self.pos = self.pos + 1;
        cast_bit
    }

    pub fn put(&mut self, byte : u8, ctx : &CriticalSectionToken) -> Result<(), ()> {
        let _ = ctx;
        if !self.is_empty() {
            Err(())
        } else {
            let stop_bit : u16 = 1 << 9;
            let parity_bit : u16 = if util::compute_parity(byte) {
                1 << 8
            } else {
                0
            };
            self.contents = (byte as u16) | parity_bit | stop_bit;
            self.pos = 0;
            Ok(())
        }
    }
}
