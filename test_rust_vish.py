#!/usr/bin/env python3
"""Build and run VISH's Rust shell or scheduler probe in a restored fixture."""

import os
import subprocess
import sys
from pathlib import Path
from typing import Iterable

ROOT = Path(__file__).resolve().parent
VISH = ROOT.parent / "vish"
ARCHIVE = ROOT / "userspace" / "initramfs.tar"
BLOB = ROOT / "userspace" / "vibix_blob.bin"
GENERATED = (
    "kernel64_entry.o", "interrupts.o", "syscall_entry.o", "context_switch.o",
    "kernel64.elf", "kernel64.bin", "boot.o", "vibix.elf",
)


def run(command, *, cwd, env=None):
    subprocess.run(command, cwd=cwd, env=env, check=True)


class ArtifactSnapshot:
    """Restore file contents, permissions, and absence after fixture builds."""

    def __init__(self, paths: Iterable[Path]):
        self.files = {
            path: (path.read_bytes(), path.stat().st_mode) if path.exists() else None
            for path in paths
        }

    def restore(self):
        for path, saved in self.files.items():
            if saved is None:
                path.unlink(missing_ok=True)
            else:
                data, mode = saved
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                path.chmod(mode)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        self.restore()


def main(kind: str) -> int:
    if kind not in ("shell", "probe"):
        raise SystemExit("usage: test_rust_vish.py shell|probe")
    if not VISH.is_dir():
        raise SystemExit(f"missing sibling VISH checkout: {VISH}")

    generated_paths = [ROOT / name for name in GENERATED]
    with ArtifactSnapshot([ARCHIVE, BLOB, *generated_paths]):
        run(["make", "elf" if kind == "shell" else "elf-probe"], cwd=VISH)
        executable = VISH / "target" / "x86_64-unknown-none" / "release" / (
            "vibix" if kind == "shell" else "vibix_probe"
        )
        if not executable.is_file() or executable.stat().st_size == 0:
            raise RuntimeError(f"missing Rust ELF artifact: {executable}")

        # Regenerate the normal VIBIT + NASM initramfs before applying the
        # isolated ELF fixture. All touched generated files are restored on exit.
        run(["make", "-B", "INIT=vibit", "all"], cwd=ROOT / "userspace")
        import io
        import tarfile

        with tarfile.open(ARCHIVE, "r") as source:
            entries = {
                member.name: (source.extractfile(member).read(), member.mode)
                for member in source.getmembers()
                if member.isfile()
            }
        entries["bin/vish"] = (executable.read_bytes(), 0o755)
        with tarfile.open(ARCHIVE, "w", format=tarfile.USTAR_FORMAT) as target:
            for name, (data, mode) in entries.items():
                info = tarfile.TarInfo(name)
                info.size = len(data)
                info.mode = mode
                info.uid = info.gid = 0
                target.addfile(info, io.BytesIO(data))

        env = os.environ.copy()
        env["VIBIX_STAGED_INITRAMFS"] = "1"
        run(["make", "-j1", "DEBUG=1", "INIT=vibit", "vibix.elf"], cwd=ROOT, env=env)
        run(["python3", "anti_cheat.py"], cwd=ROOT)
        test_name = "test_vibit_integration" if kind == "shell" else "test_vibit_rust"
        qemu = os.environ.get("QEMU", "/usr/bin/qemu-system-x86_64")
        code = (
            "import test_kernel; "
            f"test_kernel.QEMU={qemu!r}; "
            f"raise SystemExit(0 if test_kernel.{test_name}() else 1)"
        )
        run(["python3", "-c", code], cwd=ROOT, env=env)
        return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1] if len(sys.argv) == 2 else ""))
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.returncode)
