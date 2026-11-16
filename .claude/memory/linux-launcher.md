# The Linux launcher crate

Read before touching `launcher/`.

- **`launcher/` is a separate crate and must stay free of GTK, WebKit and C dependencies** (std +
  serde_json): it is built static (musl) precisely so it runs where the shell cannot load. It asks
  the system loader whether the shell beside it would load (`LD_TRACE_LOADED_OBJECTS` +
  `LD_BIND_NOW` + `LD_WARN`, i.e. `ldd -r`, ~30 ms; only when the shell's `PT_INTERP` is glibc's
  `ld-linux-*`, since under any other loader, or none, the probe would run its `main()`) and
  either `exec`s it with the same arguments or alerts (zenity → kdialog → notify-send → xmessage →
  stderr) with the packages to install per distro family, always ending by asking the user to read
  the app's documentation for its minimum requirements (the shell's floors are not the app's;
  README "System requirements" is the text apps copy). Only undefined symbols *the shell itself* needs count (it is `-z now`; a library's
  own unresolved references bind lazily), and output it does not recognise **fails open**.
  `launcher/tests/loader.rs` breaks real C programs' libraries to pin the loader's formats.
