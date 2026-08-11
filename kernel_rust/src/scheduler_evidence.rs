//! DEBUG-only correlation records for the Rust ELF scheduler probe.

#![cfg(feature = "debug")]

use crate::interrupts::InterruptFrame;
use crate::serial::SerialPort;
use core::fmt::Write;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const REQUIRED_ROUND_TRIPS: u64 = 3;

#[derive(Clone, Copy)]
struct Target {
    active: bool,
    invalid: bool,
    pid: u64,
    cr3: u64,
    rip_lo: u64,
    rip_hi: u64,
    entry: u64,
    sequence: u64,
    round_trips: u64,
    saw_user_return: bool,
    saw_switch_out: bool,
}

const EMPTY_TARGET: Target = Target {
    active: false,
    invalid: false,
    pid: 0,
    cr3: 0,
    rip_lo: 0,
    rip_hi: 0,
    entry: 0,
    sequence: 0,
    round_trips: 0,
    saw_user_return: false,
    saw_switch_out: false,
};

static mut TARGET: Target = EMPTY_TARGET;

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn executable_range(data: &[u8]) -> Option<(u64, u64)> {
    if data.len() < 64 || data.get(0..4) != Some(b"\x7fELF") || data.get(4) != Some(&2) {
        return None;
    }

    let phoff = usize::try_from(read_u64(data, 32)?).ok()?;
    let phentsize = usize::from(read_u16(data, 54)?);
    let phnum = usize::from(read_u16(data, 56)?);
    if phentsize < 56 {
        return None;
    }

    let mut rip_lo = u64::MAX;
    let mut rip_hi = 0u64;
    let mut found = false;
    for index in 0..phnum {
        let offset = phoff.checked_add(index.checked_mul(phentsize)?)?;
        let p_type = read_u32(data, offset)?;
        let p_flags = read_u32(data, offset.checked_add(4)?)?;
        if p_type != PT_LOAD || p_flags & PF_X == 0 {
            continue;
        }
        let vaddr = read_u64(data, offset.checked_add(16)?)?;
        let memsz = read_u64(data, offset.checked_add(40)?)?;
        let end = vaddr.checked_add(memsz)?;
        if memsz == 0 || end <= vaddr {
            continue;
        }
        rip_lo = core::cmp::min(rip_lo, vaddr);
        rip_hi = core::cmp::max(rip_hi, end);
        found = true;
    }

    found.then_some((rip_lo, rip_hi))
}

#[inline]
fn next_sequence(target: &mut Target) -> u64 {
    target.sequence = target.sequence.wrapping_add(1);
    target.sequence
}

#[inline]
fn frame(frame_rsp: u64) -> &'static InterruptFrame {
    unsafe { &*(frame_rsp as *const InterruptFrame) }
}

/// Register the one Rust ELF target used by the VIBIX-owned probe.
pub fn register_target(pid: u64, cr3: u64, image: &[u8], entry: u64) {
    unsafe {
        let Some((rip_lo, rip_hi)) = executable_range(image) else {
            TARGET.invalid = true;
            let mut serial = SerialPort::new();
            let _ = writeln!(
                serial,
                "DBG RUSTSCHED TARGET_INVALID pid={} reason=no-exec-range",
                pid
            );
            return;
        };

        if entry < rip_lo || entry >= rip_hi {
            TARGET.invalid = true;
            let mut serial = SerialPort::new();
            let _ = writeln!(
                serial,
                "DBG RUSTSCHED TARGET_INVALID pid={} reason=entry-out-of-range entry=0x{:016x} rip_lo=0x{:016x} rip_hi=0x{:016x}",
                pid, entry, rip_lo, rip_hi
            );
            return;
        }

        if TARGET.active {
            TARGET.invalid = true;
            let mut serial = SerialPort::new();
            let _ = writeln!(
                serial,
                "DBG RUSTSCHED TARGET_CONFLICT pid={} existing_pid={} cr3=0x{:016x} existing_cr3=0x{:016x}",
                pid,
                TARGET.pid,
                cr3,
                TARGET.cr3
            );
            return;
        }

        TARGET = Target {
            active: true,
            invalid: false,
            pid,
            cr3,
            rip_lo,
            rip_hi,
            entry,
            sequence: 0,
            round_trips: 0,
            saw_user_return: false,
            saw_switch_out: false,
        };

        let mut serial = SerialPort::new();
        let _ = writeln!(
            serial,
            "DBG RUSTSCHED TARGET pid={} cr3=0x{:016x} rip_lo=0x{:016x} rip_hi=0x{:016x} entry=0x{:016x}",
            pid, cr3, rip_lo, rip_hi, entry
        );
    }
}

/// Record the initial user handoff made by `sysretq` after ELF exec.
pub fn sysret_handoff(pid: u64, cr3: u64, rip: u64, user_rsp: u64, rflags: u64, kernel_rsp: u64) {
    unsafe {
        if !TARGET.active || TARGET.invalid || pid != TARGET.pid {
            return;
        }
        let seq = next_sequence(&mut TARGET);
        let range_ok = TARGET.rip_lo <= rip && rip < TARGET.rip_hi;
        let entry_ok = rip == TARGET.entry;
        let mut serial = SerialPort::new();
        let _ = writeln!(
            serial,
            "DBG RUSTSCHED IN seq={} round={} pid={} prev=0 cr3=0x{:016x} expected=0x{:016x} rip=0x{:016x} cs=0x23 ss=0x1b rflags=0x{:016x} rsp=0x{:016x} krsp=0x{:016x} range={} entry={} source=sysret",
            seq,
            TARGET.round_trips,
            pid,
            cr3,
            TARGET.cr3,
            rip,
            rflags,
            user_rsp,
            kernel_rsp,
            if range_ok { "ok" } else { "bad" },
            if entry_ok { "ok" } else { "bad" }
        );
    }
}

/// Record a target process being captured by a scheduler decision.
pub fn scheduled_out(pid: u64, next_pid: u64, frame_rsp: u64, active_cr3: u64) {
    unsafe {
        if !TARGET.active
            || TARGET.invalid
            || TARGET.round_trips >= REQUIRED_ROUND_TRIPS
            || pid != TARGET.pid
        {
            return;
        }
        let saved = frame(frame_rsp);
        let seq = next_sequence(&mut TARGET);
        let range_ok = TARGET.rip_lo <= saved.rip && saved.rip < TARGET.rip_hi;
        if next_pid != TARGET.pid {
            TARGET.saw_switch_out = true;
        }
        let mut serial = SerialPort::new();
        let _ = writeln!(
            serial,
            "DBG RUSTSCHED OUT seq={} round={} pid={} next={} cr3=0x{:016x} rip=0x{:016x} cs=0x{:016x} ss=0x{:016x} rflags=0x{:016x} rsp=0x{:016x} krsp=0x{:016x} range={}",
            seq,
            TARGET.round_trips,
            pid,
            next_pid,
            active_cr3,
            saved.rip,
            saved.cs,
            saved.ss,
            saved.rflags,
            saved.user_rsp,
            frame_rsp,
            if range_ok { "ok" } else { "bad" }
        );
    }
}

/// Record a target process selected after its CR3 and frame are restored.
pub fn scheduled_in(pid: u64, previous_pid: u64, frame_rsp: u64, active_cr3: u64) {
    unsafe {
        if !TARGET.active
            || TARGET.invalid
            || TARGET.round_trips >= REQUIRED_ROUND_TRIPS
            || pid != TARGET.pid
        {
            return;
        }
        let restored = frame(frame_rsp);
        let range_ok = TARGET.rip_lo <= restored.rip && restored.rip < TARGET.rip_hi;
        if previous_pid != TARGET.pid && TARGET.saw_switch_out && TARGET.saw_user_return {
            TARGET.round_trips = TARGET.round_trips.saturating_add(1);
            TARGET.saw_switch_out = false;
            TARGET.saw_user_return = false;
        }
        let seq = next_sequence(&mut TARGET);
        let mut serial = SerialPort::new();
        let _ = writeln!(
            serial,
            "DBG RUSTSCHED IN seq={} round={} pid={} prev={} cr3=0x{:016x} expected=0x{:016x} rip=0x{:016x} cs=0x{:016x} ss=0x{:016x} rflags=0x{:016x} rsp=0x{:016x} krsp=0x{:016x} range={} source=sched",
            seq,
            TARGET.round_trips,
            pid,
            previous_pid,
            active_cr3,
            TARGET.cr3,
            restored.rip,
            restored.cs,
            restored.ss,
            restored.rflags,
            restored.user_rsp,
            frame_rsp,
            if range_ok { "ok" } else { "bad" }
        );
    }
}

/// Record evidence that the target crossed an iretq/sysretq boundary.
pub fn user_return(pid: u64, frame: &InterruptFrame, active_cr3: u64) {
    unsafe {
        if !TARGET.active
            || TARGET.invalid
            || TARGET.round_trips >= REQUIRED_ROUND_TRIPS
            || pid != TARGET.pid
            || frame.cs & 3 != 3
            || frame.ss & 3 != 3
        {
            return;
        }
        TARGET.saw_user_return = true;
        let seq = next_sequence(&mut TARGET);
        let range_ok = TARGET.rip_lo <= frame.rip && frame.rip < TARGET.rip_hi;
        let mut serial = SerialPort::new();
        let _ = writeln!(
            serial,
            "DBG RUSTSCHED USER seq={} round={} pid={} cr3=0x{:016x} rip=0x{:016x} cs=0x{:016x} ss=0x{:016x} rflags=0x{:016x} rsp=0x{:016x} range={}",
            seq,
            TARGET.round_trips,
            pid,
            active_cr3,
            frame.rip,
            frame.cs,
            frame.ss,
            frame.rflags,
            frame.user_rsp,
            if range_ok { "ok" } else { "bad" }
        );
    }
}
