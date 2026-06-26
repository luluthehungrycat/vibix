//==============================================================================
// vfs/pipe.rs — Pipe implementation
//
// Provides the pipe(2) syscall via a shared ring buffer (PipeBuf) allocated
// from the kernel heap, with separate VnodeOps tables for the read and write
// ends of the pipe.
//
// Memory layout:
//   sys_pipe() allocates:
//     1 × PipeBuf (shared, refcount = 2)
//     1 × Vnode  (read end, ops = PIPE_READ_OPS,  data → PipeBuf)
//     1 × Vnode  (write end, ops = PIPE_WRITE_OPS, data → PipeBuf)
//
//   pipe_close() decrements the PipeBuf refcount and frees the vnode.
//   When refcount reaches 0, the PipeBuf is also freed.
//==============================================================================

use core::mem;
use core::ptr;
use crate::vfs::{Vnode, VnodeOps, V_CHR, ENOMEM, EINVAL, EMFILE, ESPIPE};
use crate::vfs::open_file;

//==============================================================================
// Constants
//==============================================================================

/// Pipe buffer size (Linux default).
const PIPE_BUF_SIZE: usize = 4096;

//==============================================================================
// PipeBuf — shared ring buffer
//==============================================================================

/// Shared ring buffer between the read and write ends of a pipe.
///
/// Uses absolute read/write positions that monotonically increase.
/// The actual buffer index is `position % PIPE_BUF_SIZE`.
/// One slot is kept empty to distinguish a full buffer from an empty one.
#[repr(C)]
struct PipeBuf {
    buf: [u8; PIPE_BUF_SIZE],
    /// Absolute read position (monotonically increasing).
    read_off: usize,
    /// Absolute write position (monotonically increasing).
    write_off: usize,
    /// Reference count: starts at 2 (read + write), freed when 0.
    refcount: u32,
}

//==============================================================================
// VnodeOps tables
//==============================================================================

/// Ops for the read end of a pipe: read + close.
/// write is None (reading from the write end fails with EBADF).
static PIPE_READ_OPS: VnodeOps = VnodeOps {
    open: None,
    close: Some(pipe_close as crate::vfs::VnClose),
    read: Some(pipe_read as crate::vfs::VnRead),
    write: None,
    lseek: Some(pipe_lseek as crate::vfs::VnLseek),
    readdir: None,
    ioctl: None,
};

/// Ops for the write end of a pipe: write + close.
/// read is None (writing to the read end fails with EBADF).
static PIPE_WRITE_OPS: VnodeOps = VnodeOps {
    open: None,
    close: Some(pipe_close as crate::vfs::VnClose),
    read: None,
    write: Some(pipe_write as crate::vfs::VnWrite),
    lseek: Some(pipe_lseek as crate::vfs::VnLseek),
    readdir: None,
    ioctl: None,
};

//==============================================================================
// Vnode operation implementations
//==============================================================================

/// Read from a pipe (ring buffer dequeue).
///
/// Returns the number of bytes read, or 0 if the buffer is empty.
unsafe fn pipe_read(vnode: *mut Vnode, buf: *mut u8, len: usize, _offset: &mut u64) -> isize {
    let pb = (*vnode).data as *mut PipeBuf;
    let avail = (*pb).write_off.wrapping_sub((*pb).read_off);
    if avail == 0 {
        return 0; // empty
    }
    let to_read = len.min(avail);
    for i in 0..to_read {
        *buf.add(i) = (*pb).buf[(*pb).read_off % PIPE_BUF_SIZE];
        (*pb).read_off = (*pb).read_off.wrapping_add(1);
    }
    to_read as isize
}

/// Write to a pipe (ring buffer enqueue).
///
/// Returns the number of bytes written, or 0 if the buffer is full.
unsafe fn pipe_write(vnode: *mut Vnode, buf: *const u8, len: usize, _offset: &mut u64) -> isize {
    let pb = (*vnode).data as *mut PipeBuf;
    let used = (*pb).write_off.wrapping_sub((*pb).read_off);
    // Keep one byte empty to distinguish a full buffer from an empty one.
    let space = PIPE_BUF_SIZE - 1 - used;
    if space == 0 {
        return 0; // full
    }
    let to_write = len.min(space);
    for i in 0..to_write {
        (*pb).buf[(*pb).write_off % PIPE_BUF_SIZE] = *buf.add(i);
        (*pb).write_off = (*pb).write_off.wrapping_add(1);
    }
    to_write as isize
}

/// Close one end of a pipe.
///
/// Decrements the PipeBuf refcount. If it reaches 0, the PipeBuf is freed.
/// Always frees the dynamically-allocated vnode itself.
///
/// # Safety
///
/// The vnode must have been allocated by kmalloc in sys_pipe.
/// After close returns, the calling OFT entry will be set to None
/// (no further access to the vnode pointer).
unsafe fn pipe_close(vnode: *mut Vnode) -> i32 {
    let pb = (*vnode).data as *mut PipeBuf;
    (*pb).refcount = (*pb).refcount.wrapping_sub(1);
    let need_free_pb = (*pb).refcount == 0;
    // Free the dynamically-allocated vnode itself.
    crate::kmm::kfree(vnode as *mut u8);
    if need_free_pb {
        crate::kmm::kfree(pb as *mut u8);
    }
    0
}

/// lseek on a pipe — always returns -ESPIPE (illegal seek).
unsafe fn pipe_lseek(_vnode: *mut Vnode, _offset: i64, _whence: u32) -> i64 {
    -(ESPIPE as i64)
}

//==============================================================================
// Public API — sys_pipe
//==============================================================================

/// Create a pipe.
///
/// `pipefd` is a pointer to a user-space array of two `i32`s.
/// On success, writes `[read_fd, write_fd]` to `pipefd` and returns 0.
/// On error, returns a negative errno value.
///
/// # Safety
///
/// This function writes to the user-space pointer `pipefd`.
/// It must only be called from the syscall handler with interrupts disabled.
pub fn sys_pipe(pipefd: u64) -> i64 {
    if pipefd == 0 {
        return -(EINVAL as i64);
    }

    unsafe {
        //==========================================================================
        // 1. Allocate the shared PipeBuf
        //==========================================================================
        let pb_ptr = crate::kmm::kmalloc(mem::size_of::<PipeBuf>());
        if pb_ptr.is_null() {
            return -(ENOMEM as i64);
        }
        ptr::write_bytes(pb_ptr, 0, mem::size_of::<PipeBuf>());
        let pb = pb_ptr as *mut PipeBuf;
        (*pb).refcount = 2;

        //==========================================================================
        // 2. Allocate the read-end Vnode
        //==========================================================================
        let rvn_ptr = crate::kmm::kmalloc(mem::size_of::<Vnode>());
        if rvn_ptr.is_null() {
            crate::kmm::kfree(pb_ptr);
            return -(ENOMEM as i64);
        }
        ptr::write_bytes(rvn_ptr, 0, mem::size_of::<Vnode>());
        let rvn = &mut *(rvn_ptr as *mut Vnode);
        *rvn = Vnode {
            ops: core::ptr::addr_of!(PIPE_READ_OPS) as *const VnodeOps,
            mode: V_CHR | 0o600,
            ino: 0,
            size: 0,
            fs_type: crate::vfs::FsType::DevFS,
            data: pb as *mut (),
            mount: None,
        };

        //==========================================================================
        // 3. Allocate the write-end Vnode
        //==========================================================================
        let wvn_ptr = crate::kmm::kmalloc(mem::size_of::<Vnode>());
        if wvn_ptr.is_null() {
            crate::kmm::kfree(pb_ptr);
            crate::kmm::kfree(rvn_ptr);
            return -(ENOMEM as i64);
        }
        ptr::write_bytes(wvn_ptr, 0, mem::size_of::<Vnode>());
        let wvn = &mut *(wvn_ptr as *mut Vnode);
        *wvn = Vnode {
            ops: core::ptr::addr_of!(PIPE_WRITE_OPS) as *const VnodeOps,
            mode: V_CHR | 0o600,
            ino: 0,
            size: 0,
            fs_type: crate::vfs::FsType::DevFS,
            data: pb as *mut (),
            mount: None,
        };

        //==========================================================================
        // 4. Allocate read-end OFT entry
        //==========================================================================
        let read_idx = match open_file::oft_alloc(&mut *rvn, crate::vfs::O_RDONLY, 0o600) {
            Ok(idx) => idx,
            Err(e) => {
                crate::kmm::kfree(pb_ptr);
                crate::kmm::kfree(rvn_ptr);
                crate::kmm::kfree(wvn_ptr);
                return -(e as i64);
            }
        };

        //==========================================================================
        // 5. Allocate write-end OFT entry
        //==========================================================================
        let write_idx = match open_file::oft_alloc(&mut *wvn, crate::vfs::O_WRONLY, 0o600) {
            Ok(idx) => idx,
            Err(e) => {
                // Read OFT entry is already allocated — close it.
                // This calls pipe_close on rvn, which:
                //   - decrements PipeBuf refcount to 1
                //   - kfree's rvn_ptr
                open_file::oft_decref(read_idx);
                // PipeBuf refcount is now 1 (stale — no remaining references).
                // Free the PipeBuf and the unpublished write vnode.
                crate::kmm::kfree(pb_ptr);
                crate::kmm::kfree(wvn_ptr);
                // rvn_ptr was freed by pipe_close — do NOT free it again.
                return -(e as i64);
            }
        };

        //==========================================================================
        // 6. Find free file descriptors in the current process's fd table
        //==========================================================================
        let fds = &mut crate::process::process_mut(crate::process::current_pid()).fd_table.fds;

        // Find free slot for the read end
        let read_fd = match fds.iter().position(|&fd| fd < 0) {
            Some(pos) => {
                fds[pos] = read_idx as i32;
                pos as u32
            }
            None => {
                // No free fd — rollback both OFT entries.
                // oft_decref on both will free vnodes and PipeBuf.
                open_file::oft_decref(read_idx);
                open_file::oft_decref(write_idx);
                return -(EMFILE as i64);
            }
        };

        // Find free slot for the write end
        let write_fd = match fds.iter().position(|&fd| fd < 0) {
            Some(pos) => {
                fds[pos] = write_idx as i32;
                pos as u32
            }
            None => {
                // Rollback the read fd and both OFT entries.
                fds[read_fd as usize] = -1;
                open_file::oft_decref(read_idx);
                open_file::oft_decref(write_idx);
                return -(EMFILE as i64);
            }
        };

        //==========================================================================
        // 7. Write fds to user-space buffer
        //==========================================================================
        let user_buf = pipefd as *mut u32;
        ptr::write(user_buf, read_fd);
        ptr::write(user_buf.add(1), write_fd);
    }

    0
}
