# VIBIX

UNIXoid kernel written from scratch using coding agent workflows. MVP focuses on QEMU/KVM compatibility only.


![VIBIX Logo](https://via.placeholder.com/150) *(Replace with a real logo later!)*

---

## **📌 Overview**
VIBIX is a **minimal UNIX-like kernel** developed with the help of coding agents (Vibe CLI, Web Vibe, OpenCode, Hermes Agent) and traditional coding. It targets **QEMU/KVM** first, with plans to expand to VirtualBox and bare metal.

**Goals**:
- ✅ Bootable kernel with serial debugging.
- ✅ Physical Memory Management (PMM).
- ✅ Minimal shell (`vish`).
- 🚧 Interrupts, timers, and keyboard input.
- 🚀 Future: Process management, syscalls, file systems.

**Ecosystem**:
- **GVIBU**: A busybox-style coreutils replacement (separate project).
- **GVIBU/VIBIX**: The combined userspace + kernel system.

---

## **🛠️ Setup**
### **Prerequisites**
- **Toolchain**: `nasm`, `gcc`, `ld`, `make`, `qemu-system-x86_64`.
- **Python**: For testing and anti-cheat scripts.
- **Git**: For version control.

### **Install Dependencies (Arch Linux)**
```bash
sudo pacman -S nasm gcc make qemu python git
