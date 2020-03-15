MODE := "release"
XFLAGS := "--release"
TARGET := "target/msp430-none-elf/" + MODE + "/at2xt"

# Build AT2XT using the timer feature.
timer:
    xargo build {{XFLAGS}} --target=msp430-none-elf
    msp430-elf-objdump -Cd {{TARGET}} > {{TARGET}}.lst
    msp430-elf-readelf -s --wide {{TARGET}} > {{TARGET}}.sym
    msp430-elf-size {{TARGET}}

# Build AT2XT using the timer feature and extra artifacts.
timer-extra:
    xargo rustc {{XFLAGS}} --target=msp430-none-elf -- --emit=obj={{TARGET}}.o
    msp430-elf-objdump -Cd {{TARGET}} > {{TARGET}}.lst
    msp430-elf-readelf -s --wide {{TARGET}} > {{TARGET}}.sym
    msp430-elf-objdump -Cd {{TARGET}}.o > {{TARGET}}.o.lst
    msp430-elf-readelf -r --wide {{TARGET}}.o > {{TARGET}}.reloc
    msp430-elf-size {{TARGET}}

fix:
  xargo fix --target=msp430-none-elf

# Remove AT2XT and dependencies.
clean:
    xargo clean

# Upload firmware to AT2XT board using MSP-EXP430G2.
prog:
    mspdebug rf2500 "prog {{TARGET}}"
