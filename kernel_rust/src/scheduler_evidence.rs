//! DEBUG-only correlation records for the Rust ELF scheduler probe.

#![cfg(feature = "debug")]

use crate::interrupts::InterruptFrame;
use crate::serial::SerialPort;
use core::fmt::Write;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const REQUIRED_ROUND_TRIPS: u64 = 3;
const MAX_EXEC_INTERVALS: usize = 64;

#[derive(Clone, Copy)]
struct ExecInterval {
    lo: u64,
    hi: u64,
}

const EMPTY_INTERVAL: ExecInterval = ExecInterval { lo: 0, hi: 0 };

#[derive(Clone, Copy)]
struct Target {
    active: bool,
    invalid: bool,
    pid: u64,
    cr3: u64,
    intervals: [ExecInterval; MAX_EXEC_INTERVALS],
    interval_count: usize,
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
    intervals: [EMPTY_INTERVAL; MAX_EXEC_INTERVALS],
    interval_count: 0,
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

#[derive(Clone, Copy)]
enum IntervalError {
    Malformed,
    Capacity,
}

fn executable_intervals(
    data: &[u8],
) -> Result<([ExecInterval; MAX_EXEC_INTERVALS], usize), IntervalError> {
    if data.len() < 64 || data.get(0..4) != Some(b"\x7fELF") || data.get(4) != Some(&2) {
        return Err(IntervalError::Malformed);
    }

    let phoff = usize::try_from(read_u64(data, 32).ok_or(IntervalError::Malformed)?)
        .map_err(|_| IntervalError::Malformed)?;
    let phentsize = usize::from(read_u16(data, 54).ok_or(IntervalError::Malformed)?);
    let phnum = usize::from(read_u16(data, 56).ok_or(IntervalError::Malformed)?);
    if phentsize < 56 {
        return Err(IntervalError::Malformed);
    }

    let mut intervals = [EMPTY_INTERVAL; MAX_EXEC_INTERVALS];
    let mut count = 0usize;
    for index in 0..phnum {
        let offset = phoff
            .checked_add(index.checked_mul(phentsize).ok_or(IntervalError::Malformed)?)
            .ok_or(IntervalError::Malformed)?;
        let p_type = read_u32(data, offset).ok_or(IntervalError::Malformed)?;
        let p_flags = read_u32(data, offset.checked_add(4).ok_or(IntervalError::Malformed)?)
            .ok_or(IntervalError::Malformed)?;
        if p_type != PT_LOAD || p_flags & PF_X == 0 {
            continue;
        }
        let vaddr = read_u64(data, offset.checked_add(16).ok_or(IntervalError::Malformed)?)
            .ok_or(IntervalError::Malformed)?;
        let memsz = read_u64(data, offset.checked_add(40).ok_or(IntervalError::Malformed)?)
            .ok_or(IntervalError::Malformed)?;
        let end = vaddr.checked_add(memsz).ok_or(IntervalError::Malformed)?;
        if memsz == 0 || end <= vaddr {
            continue;
        }
        let mut lo = vaddr;
        let mut hi = end;
        let mut first = 0usize;
        while first < count && intervals[first].hi < lo {
            first += 1;
        }
        let mut last = first;
        while last < count && intervals[last].lo <= hi {
            lo = lo.min(intervals[last].lo);
            hi = hi.max(intervals[last].hi);
            last += 1;
        }
        if first == last {
            if count == MAX_EXEC_INTERVALS {
                return Err(IntervalError::Capacity);
            }
            for index in (first..count).rev() {
                intervals[index + 1] = intervals[index];
            }
            intervals[first] = ExecInterval { lo, hi };
            count += 1;
        } else {
            intervals[first] = ExecInterval { lo, hi };
            let removed = last - first - 1;
            for index in last..count {
                intervals[index - removed] = intervals[index];
            }
            count -= removed;
        }
    }

    if count == 0 {
        Err(IntervalError::Malformed)
    } else {
        Ok((intervals, count))
    }
}

#[inline]
fn rip_in_intervals(target: &Target, rip: u64) -> bool {
    target.intervals[..target.interval_count]
        .iter()
        .any(|interval| interval.lo <= rip && rip < interval.hi)
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
        let parsed = executable_intervals(image);
        let (intervals, interval_count) = match parsed {
            Ok(value) => value,
            Err(IntervalError::Capacity) => {
                TARGET.invalid = true;
                let mut serial = SerialPort::new();
                let _ = writeln!(
                    serial,
                    "DBG RUSTSCHED TARGET_INVALID pid={} reason=interval-capacity max={}",
                    pid, MAX_EXEC_INTERVALS
                );
                return;
            }
            Err(IntervalError::Malformed) => {
                TARGET.invalid = true;
                let mut serial = SerialPort::new();
                let _ = writeln!(
                    serial,
                    "DBG RUSTSCHED TARGET_INVALID pid={} reason=no-exec-range",
                    pid
                );
                return;
            }
        };

        if !intervals[..interval_count]
            .iter()
            .any(|interval| interval.lo <= entry && entry < interval.hi)
        {
            TARGET.invalid = true;
            let mut serial = SerialPort::new();
            let _ = writeln!(
                serial,
                "DBG RUSTSCHED TARGET_INVALID pid={} reason=entry-out-of-range entry=0x{:016x}",
                pid, entry
            );
            return;
        }

        // A lifecycle probe may exec the same path again after the first
        // synthetic child has completed its required evidence.  Keep the
        // first completed transcript authoritative and ignore later targets;
        // replacing it would create a false TARGET_CONFLICT in one serial
        // transcript.
        if TARGET.active && TARGET.round_trips >= REQUIRED_ROUND_TRIPS {
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
            intervals,
            interval_count,
            entry,
            sequence: 0,
            round_trips: 0,
            saw_user_return: false,
            saw_switch_out: false,
        };

        let mut serial = SerialPort::new();
        let _ = write!(
            serial,
            "DBG RUSTSCHED TARGET pid={} cr3=0x{:016x} interval_count={}",
            pid, cr3, interval_count
        );
        for (index, interval) in intervals[..interval_count].iter().enumerate() {
            let _ = write!(serial, " interval{}_lo=0x{:016x} interval{}_hi=0x{:016x}", index, interval.lo, index, interval.hi);
        }
        let _ = writeln!(serial, " entry=0x{:016x}", entry);
    }
}

/// Record the initial user handoff made by `sysretq` after ELF exec.
pub fn sysret_handoff(pid: u64, cr3: u64, rip: u64, user_rsp: u64, rflags: u64, kernel_rsp: u64) {
    unsafe {
        if !TARGET.active || TARGET.invalid || pid != TARGET.pid {
            return;
        }
        let seq = next_sequence(&mut TARGET);
        let range_ok = rip_in_intervals(&TARGET, rip);
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
        let range_ok = rip_in_intervals(&TARGET, saved.rip);
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
        let range_ok = rip_in_intervals(&TARGET, restored.rip);
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
        let range_ok = rip_in_intervals(&TARGET, frame.rip);
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
