#!/bin/sh
exec /usr/bin/qemu-system-x86_64 \
    -accel tcg \
    -kernel vibix.elf \
    -serial stdio \
    -display none \
    -m 512M \
    -smp 1 \
    -no-reboot \
    -no-shutdown \
    -d int,cpu_reset 2>&1 | tee qemu.log
