# 0011: Linux UI and video: GTK4 + libadwaita on the core directly, libmpv's render API into a GtkGLArea (SDR), HDR later

Status: proposed (2026-10-22), from the D8 spike; accepted when its PR merges. Evidence, sources and the hands-on list:
`docs/notes/linux-gtk.md`.

## Context
ADR 0001 and ADR 0002 give Linux a GTK4 + libadwaita app in Rust that uses the core directly, and docs/architecture.md picks libmpv in a
`GtkGLArea` as its engine. D8 built a throwaway prototype on `current-app`, with libmpv 0.37 (Ubuntu 24.04) rendering a synthetic
clip into a GLArea under GTK controls. It ran only headless in a cloud container on software GL (Mesa llvmpipe; Xvfb and weston
headless).

The owner then ran it in WSL2 (WSLg, Ubuntu 24.04) on an NVIDIA RTX 5090:
- **The GPU path had to be forced.** Mesa does not pick the GPU on its own there: it logs "ZINK: failed to choose pdev" and falls
  back to llvmpipe. `GALLIUM_DRIVER=d3d12` with `MESA_D3D12_DEFAULT_ADAPTER_NAME=NVIDIA` gives Mesa's d3d12 driver (GL 4.6) on the
  NVIDIA GPU.
- **On the GPU the picture was right:** correct colours and never green. Resizing and full screen passed on both GLES and desktop GL.
- **Preferences and About in the primary menu** feel right for GNOME to the owner.

Native Linux checks are still to come.

## Decision

### UI: gtk4-rs and libadwaita, laid out the GNOME way
- **Crates:** gtk4-rs and libadwaita-rs (MIT). The prototype pinned `gtk4 =0.11.5` and `libadwaita =0.9.2` with features `v4_14` and
  `v1_5`, approved for the prototype only. M5 asks again for the versions current then.
- **Minimum system versions** (stated as versions, not distro packages):
  - GTK 4.14 and libadwaita 1.5 (both in Ubuntu 24.04 LTS).
  - libmpv soname 2 (mpv 0.35 or later), reached through EGL with GL 3.x or GLES 3.x.
  - The glibc of the oldest build host we support; the spike's binary built on Ubuntu 24.04 needs 2.34 (the highest `GLIBC_` symbol version in `objdump -T <binary>`).
  - A Flatpak build would use the runtime's newer GTK instead.
- **Layout:**
  - `AdwApplicationWindow` with an `AdwOverlaySplitView`: a sidebar of sections, a header bar per pane, and a breakpoint that folds
    the sidebar on narrow windows.
  - Settings and About are not sidebar entries as on Windows. They go in the primary menu as Preferences (Ctrl+,,
    `AdwPreferencesDialog`) and About (`AdwAboutDialog`), where GNOME apps put them (ADR 0001: platform behaviour wins).
  - The owner confirms this on the WSL2 run.
- **Core:** the app owns one `App` and one tokio runtime, as `current-ffi` does.
  - Observable values are awaited directly on the GLib main context (`glib::spawn_future_local` over `tokio::sync::watch`); tokio's
    channels need no tokio runtime to be awaited.
  - Lists use `current-app`'s `ListSubscription` the same way and apply the diffs to a `gio::ListStore`. This is ADR 0004's
    watch/diff model without UniFFI. The spike built the value path only; lists are untested. GTK objects never leave the main thread.

### Video: libmpv's OpenGL render API into a GtkGLArea, SDR
- **Route:** `vo=libmpv`, `mpv_render_context_create` with `MPV_RENDER_API_TYPE_OPENGL` in the GLArea's context. The code is in
  `current-mpv`, M2's crate, which ADR 0002 already allows `unsafe` for.
  - `get_proc_address` comes from `eglGetProcAddress`. GTK uses EGL on Wayland and X11 (GLX only as a fallback; GTK NEWS 4.3.1).
  - Rendering happens in the GLArea's `render` signal into its bound framebuffer, at widget size times scale factor, with
    `MPV_RENDER_PARAM_FLIP_Y`. GTK's framebuffer is rebound afterwards, because mpv leaves 0 bound.
  - mpv's update and wakeup callbacks only wake the main loop; nothing on it waits for mpv.
  - GTK's default API choice stands (GLES since GTK 4.14); mpv works on both.
- **Controls:** ours, as GTK widgets in a `GtkOverlay` over the GLArea, auto-hiding with the pointer (ADR 0001).
- **The ADR 0007 idea carries over:** the UI reports facts, and `current-mpv` maps them to mpv options.
  - On Linux the facts are the monitor's refresh rate (`GdkMonitor`, giving `display-fps-override`) and the GL renderer.
  - A software rasteriser (llvmpipe, softpipe, swrast) gets `fbo-format=rgba8`: with mpv's default `rgba16f`, about half the runs
    drew black on llvmpipe in the spike, and `rgba8` fixed every run.
  - HDR facts are reported as "SDR" until an HDR path exists, so mpv tone-maps HDR sources to SDR.
- **Hardware decoding:** `hwdec=auto-safe`. VA-API with the render API needs EGL and the native display passed in
  (`MPV_RENDER_PARAM_WL_DISPLAY` / `X11_DISPLAY`, render_gl.h), which needs gtk-rs's `gdk4-wayland` and `gdk4-x11` crates (owner's
  approval in M5). NVDEC works with GL. `vulkan` hwdec needs `vo=gpu-next` and is out of reach of the render API. Untested: there was
  no GPU.

### HDR: not in M5's first version; the paths to watch
None of today's pieces carries HDR from mpv into a GTK window (sources read 2026-10-22):
1. **The libmpv render API renders with mpv's older `vo_gpu` renderer** and has no output colour space parameter. `--target-colorspace-hint`
   "Requires ... `--vo=gpu-next`".
2. **GtkGLArea always allocates 8-bit `GL_RGBA8` textures** without a colour state (gtkglarea.c, 4.24.1).
3. **GTK only uses the Wayland colour-management protocol behind `GDK_DEBUG=color-mgmt`** (still opt-in in 4.24.1). The protocol itself
   is still in wayland-protocols' `staging/`.
4. **X11 has no HDR at all.**

Compositors are ready: GNOME 48 or later, Plasma 6.4 or later, wlroots 0.19 or later. Candidate paths, in order of preference; each
needs its own spike once the blocking piece moves:
- **(a) GTK route:** mpv renders into our own 10-bit or float texture with PQ/BT.2020 target options. We wrap it with
  `GdkGLTextureBuilder` or `GdkDmabufTextureBuilder` and a rec2100-PQ colour state, and show it through `GtkGraphicsOffload` (Wayland
  only), once GTK enables colour management by default. This depends on whether `vo_gpu`'s `target-trc`/`target-prim` work through
  the render API; mpv's documentation does not say.
- **(b) mpv route:** a future mpv render API on `gpu-next` (only D3D11 has been added, for the unreleased 0.42), or mpv drawing into
  its own Wayland subsurface of our window.

Linux is SDR until then; the roadmap's M5 says so.

### Desktop integration (M5, and M1 for the keyring)
- **Media keys and the desktop's media controls through MPRIS 2.2,** implemented with gio's D-Bus (gtk4-rs re-exports gio, so no new
  crate). The bus name is `org.mpris.MediaPlayer2.<app id>`, which Flatpak's default policy allows. The `mpris-server` crate is
  MPL-2.0 and brings a second D-Bus stack (zbus), so it is not chosen.
- **Secrets:** the Secret Service through the `keyring` crate (ADR 0002) on a host install.
  - In Flatpak that crate bypasses the Secret portal and would need access to `org.freedesktop.secrets`.
  - M1 or M5 decides between that and the portal plus our own encrypted store.
- **Portals:** GTK uses them by itself in a sandbox (file chooser, open URI, settings). Playback inhibits idle with
  `gtk_application_inhibit`.
- **Accessibility:** GTK 4's accessible roles and labels reach Orca through AT-SPI, which GTK still defaults to (it added an
  AccessKit backend in 4.18). Every control gets a label, as for Narrator on Windows.

### Distribution: libmpv and FFmpeg
- **Distro libraries are GPL builds.** Ubuntu 24.04's and Debian's FFmpeg are built with `--enable-gpl`, and Fedora's `ffmpeg-free`
  is GPL-3.0-or-later. Ubuntu's and Debian's libmpv is built with GPL-only features (cdda, dvdnav, dvbin), so it is GPL. A
  proprietary app linked against them is outside ADR 0003. GTK, libadwaita and GLib are LGPL-2.1-or-later and fine as system
  libraries.
- **The spike and development builds** use the system libmpv; nothing is distributed. **A distributed build needs our own LGPL-2.1
  libmpv (`-Dgpl=false`) and FFmpeg,** as D6 built for Windows, kept as separate shared libraries (replaceable, per ADR 0003). In mpv 0.37's
  `meson.build` an LGPL build also has no X11 window backend, which the render API does not need.
- **How to ship: options, for the owner to decide before M5's release work.** "Distro independence" means one build for every
  distribution.

| Option | libmpv/FFmpeg | GTK/libadwaita | Distro independence | Cost |
|---|---|---|---|---|
| Flatpak (GNOME runtime, Flathub or our own repo) | Ours, built in the manifest; the runtime's FFmpeg is LGPL but lacks H.264/HEVC, and its `codecs-extra` is GPL | The runtime's (GNOME 51 is built on freedesktop-sdk 26.08) | Yes: one build for every distro with Flatpak | A manifest and our libmpv/FFmpeg modules; the sandbox's rules for secrets (above); Flathub requires redistributable content |
| Per-distro packages (.deb, .rpm, Arch) | Ours, installed beside the app (the distro's are GPL) | The distro's, at least 4.14 / 1.5 | No: one build per distro family and release | Several builds and repos, each tested; dependency names differ (Ubuntu `libgtk-4-1`, Fedora `gtk4`, Arch `gtk4`) |
| AppImage | Ours, bundled | Bundled, or the host's (bundling GTK is fragile with themes and GL drivers) | Mostly: needs a glibc and GL stack at least as new as the build host | Bundling GTK and keeping it working across distros; no sandbox; updates are ours |
| Plain tarball | Ours, bundled | The host's, at least 4.14 / 1.5 | Partly: any distro with new enough GTK, libadwaita and glibc | Users install dependencies themselves; no updates, no desktop integration unless installed by a script |

Snap is left out (one more store, with its own sandbox and runtime).

### CI for M5 (proposal; no job is added now)
- **apps/linux joins the root workspace,** with GTK crates declared in `[workspace.dependencies]`. Windows and macOS jobs must not
  build it: `default-members` without it there, or `--exclude current-linux` per job.
- **Linux job:**
  1. Install the GTK4, libadwaita and libmpv dev packages, plus Xvfb (or weston), Mesa and ffmpeg.
  2. Build, clippy and test the app.
  3. Generate the synthetic clip.
  4. Run a headless smoke run under Xvfb with `GDK_BACKEND=x11`: the window opens, the core state reaches it, and mpv reports
     rendered frames and a moving position. Pixel colours are not checked, because llvmpipe's picture is unreliable (notes).
- **cargo-deny** covers the gtk-rs tree (all MIT/Apache-2.0 today).
- **Later:** a job that builds our LGPL libmpv and FFmpeg for Linux and fails on GPL components, as D6 does for Windows. A Flatpak
  build job only if Flatpak is chosen.

## Consequences
- **From the WSL2 run, for M5** (prototype bugs; M5 owns keyboard handling):
  - Space must be play/pause at the player level, not activate the focused button.
  - Left/Right seeked twice per press.
  - Dragging the seek bar lagged.
  - Ctrl shortcuts did nothing under `GDK_BACKEND=x11`.
- **WSLg is not a performance or X11 reference.** WSLg only, not verified on native Linux:
  - The d3d12 path was very slow: the player page took about 30 s to load.
  - The X11 (XWayland) window was transparent, with DRI3 and x11 shm errors.
  - Native Linux decides both.
- Linux plays SDR only at first. HDR waits for GTK and mpv, and the roadmap carries it as a follow-up.
- M2's `current-mpv` serves two render targets: D3D11 composition on Windows, and the OpenGL render API on Linux. Both share the
  display-facts mapping.
- M5 needs approval for gtk-rs's `gdk4-wayland`/`gdk4-x11` crates (VA-API) and for the GTK crate versions current then.
- Software-GL artefacts on llvmpipe (green picture, broken downscales; also in mpv's own window) mean headless tests can only check
  that frames are rendered, not what they look like.
