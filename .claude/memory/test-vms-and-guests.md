# Test VMs and guests

Read before testing a packaged app on a clean Linux, macOS or Windows machine.

Read `docs/adr/0001-shell-stack.md` before changing the webview glue. To test a packaged app on clean
Linux machines, use `scripts/dev/linuxvm.sh` (`docs/dev/linux-test-vms.md`), which also drives the
guests' real pointer (QMP, absolute: `move`/`click`/`drag`) for what the automation channel only
fakes. Its `ubuntu-22.04-arm64` guest is emulated and headless: there is no arm64 hardware, so the arm64
tarball's worker, headless launch and `install.sh` are checked there. On clean macOS 12 and 27 guests, `scripts/dev/macvm.sh` (`docs/dev/macos-test-vms.md`). To
run anything on Windows, use the test VMs in `docs/dev/windows-test-vm.md` through
`scripts/dev/winvm.sh` (`ssh winvm`, `winvm10`). A GUI program must be started in the desktop
session (`winvm.sh run`, an interactive scheduled task), never directly over SSH; the automation
channel reaches the host through `winvm.sh tunnel`.
