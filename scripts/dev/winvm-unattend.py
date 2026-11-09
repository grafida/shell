#!/usr/bin/env python3
# Grafida Shell — development tooling.
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT licence; see LICENSE.
"""Add SSH access to a quickemu Windows VM's unattended install, then rebuild unattended.iso.

usage: winvm-unattend.py <vm-dir> <public-key-file>
       e.g. winvm-unattend.py ~/VMs/windows-11 ~/.ssh/grafida-winvm_ed25519.pub

Run it after `quickget windows 11` and BEFORE the first `quickemu --vm …` boot. It appends
FirstLogonCommands to <vm-dir>/unattended/autounattend.xml (keeping the original as
autounattend.xml.orig) and re-runs the same mkisofs command quickget uses. Running it twice is a
no-op. See docs/dev/windows-test-vm.md.
"""
import pathlib
import re
import shutil
import subprocess
import sys
from xml.sax.saxutils import escape

MARKER = "grafida-winvm"


def ps(command: str) -> str:
    return f'powershell -NoProfile -ExecutionPolicy Bypass -Command "{command}"'


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__.strip(), file=sys.stderr)
        return 64
    vm = pathlib.Path(sys.argv[1]).expanduser()
    pub = pathlib.Path(sys.argv[2]).expanduser().read_text().strip()
    if not pub.startswith("ssh-") or "'" in pub or "\n" in pub:
        print("not a single-line OpenSSH public key", file=sys.stderr)
        return 65
    xml = vm / "unattended" / "autounattend.xml"
    text = xml.read_text()
    if MARKER in text:
        print(f"{xml}: already patched")
        return 0

    keys = r"C:\ProgramData\ssh\administrators_authorized_keys"
    commands = [
        # The OpenSSH firewall rule is Private-only, but Windows 11 puts the VM's network in
        # Public regardless of the answer file's NetworkLocation, so widen it.
        ("Install and start OpenSSH Server",
         ps("Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0; "
            "Set-Service sshd -StartupType Automatic; Start-Service sshd; "
            "Set-NetFirewallRule -Name OpenSSH-Server-In-TCP -Profile Any")),
        # Members of Administrators read this file, not ~\.ssh\authorized_keys; sshd refuses it
        # unless only Administrators and SYSTEM can access it.
        ("Authorise the host key for the Administrators group",
         ps(r"New-Item -ItemType Directory -Force C:\ProgramData\ssh | Out-Null; "
            f"Set-Content -Encoding ascii -Path {keys} -Value '{pub}'; "
            f"icacls {keys} /inheritance:r /grant Administrators:F /grant SYSTEM:F")),
        ("Use Windows PowerShell as the SSH shell",
         r"reg add HKLM\SOFTWARE\OpenSSH /v DefaultShell /t REG_SZ /d "
         r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe /f"),
        ("Never sleep or blank the display",
         "cmd /c powercfg /change standby-timeout-ac 0 && powercfg /change monitor-timeout-ac 0"),
        ("No lock screen",
         r"reg add HKLM\SOFTWARE\Policies\Microsoft\Windows\Personalization /v NoLockScreen /t REG_DWORD /d 1 /f"),
        ("Restart sshd so it picks up the key and shell", ps("Restart-Service sshd")),
    ]
    anchor = "      </FirstLogonCommands>"
    if text.count(anchor) != 1:
        print(f"{xml}: unexpected layout (no single FirstLogonCommands block)", file=sys.stderr)
        return 65
    last = max(int(o) for o in re.findall(r"<Order>(\d+)</Order>", text.split("<FirstLogonCommands>")[1]))
    block = ""
    for order, (description, command) in enumerate(commands, start=last + 1):
        block += (
            '        <SynchronousCommand wcm:action="add">\n'
            f"          <CommandLine>{escape(command)}</CommandLine>\n"
            f"          <Description>{escape(description)} ({MARKER})</Description>\n"
            f"          <Order>{order}</Order>\n"
            "        </SynchronousCommand>\n"
        )
    shutil.copyfile(xml, xml.with_name("autounattend.xml.orig"))
    xml.write_text(text.replace(anchor, block + anchor))
    iso = vm / "unattended.iso"
    iso.unlink(missing_ok=True)
    subprocess.run(["mkisofs", "-quiet", "-J", "-o", str(iso), str(vm / "unattended")], check=True)
    print(f"patched {xml} and rebuilt {iso}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
