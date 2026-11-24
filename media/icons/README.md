# Icons

`icon-master.png` is the transparent 1024×1024 master (knocked out from the
original artwork, despeckled, centred). Every other file is generated from it;
to regenerate, rescale the master with Lanczos and repackage:

| File | Purpose |
| --- | --- |
| `icon-16.png` … `icon-512.png` | Linux desktop (hicolor icon theme: `~/.local/share/icons/hicolor/<N>x<N>/apps/re-demo.png`) and general use |
| `icon.ico` | Windows icon resource container (PNG-encoded frames, 16–256) |
| `icon.res` | Compiled Win32 resource (`RT_ICON` + `RT_GROUP_ICON`), linked into `re-demo.exe` by `crates/re-wgpu/build.rs` on MSVC targets |
| `icon-128.rgba` | Raw RGBA8, embedded with `include_bytes!` by `re-platform` as the runtime window/taskbar icon on every platform |

*Deutsch:* `icon-master.png` ist die transparente 1024×1024-Vorlage; alle
anderen Dateien werden daraus erzeugt. Die `.res` verlinkt der Build der
Windows-EXE, das `.rgba` bettet `re-platform` als Fenster-Icon ein, die PNGs
sind für das Linux-Icon-Theme (hicolor) bestimmt.
