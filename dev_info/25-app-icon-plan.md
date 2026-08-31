# 25 — The app icon: nothing to pull, something to install

Status: plan. Measured against the tree at `a29826d` and against this machine.

Its own file rather than a section of [24](24-annotation-style-plan.md): that
one is about how a mark is drawn, this is about packaging and desktop
integration. They share nothing but the word "icon".

## The finding: the icon is fine

There is a complete, correct icon set already:

```
assets/icons/hicolor/{16,24,32,48,64,128,256,512}x*/apps/net.canfar.Verbinal.png
assets/icons/hicolor/scalable/apps/net.canfar.Verbinal.svg   (a real vector)
data/net.canfar.Verbinal.desktop                             Icon=net.canfar.Verbinal
src/lib.rs                                                   application_id("net.canfar.Verbinal")
Cargo.toml [package.metadata.deb]                            ships all ten
```

The artwork is the Verbinal mark — blue ring, cyan dot, dotted inner ring, dark
V — and it is **the same design as `CanfarDesktop/Assets/Verbinal.ico`**,
compared side by side at 256px. The Linux one is a proper SVG rather than a
raster `.ico`, so it is the better copy of the two.

**So there is nothing to pull from the Windows app, and macOS does not come
into it** — that is not a target here.

## Why you see a cog

The icons are not installed on this machine:

```
/usr/share/icons/hicolor/256x256/apps/net.canfar.Verbinal.png   absent
/usr/share/applications/net.canfar.Verbinal.desktop             absent
dpkg -l verbinal                                                not installed
```

Running `./target/release/verbinal` from the build tree, the shell has no
`.desktop` file to match the window's `app_id` against, so it falls back to a
generic icon. That is the cog.

**`src/lib.rs` does add an icon search path**, pointing at
`CARGO_MANIFEST_DIR/assets/icons`, and that makes the icon available *inside*
the process — to `AboutWindow`, which asks the GTK icon theme by name. It
cannot help the window's own icon: on Wayland the shell resolves that through
the installed desktop entry, not through the application's icon theme. Two
different lookups, and only one of them is in the app's gift.

## What is actually worth doing

### 1. A dev install, so a source build looks like the real thing

A small script installing the desktop entry and the icons into
`~/.local/share/`, and running `update-desktop-database` and
`gtk4-update-icon-cache`. Twenty lines, and it makes the difference between
"the app looks unfinished" and "the app looks like an app" for anyone building
from source — which is everyone working on it.

Uninstall too, or it becomes something people are afraid to run.

### 2. `CARGO_MANIFEST_DIR` in a shipped binary

That search path is an absolute path from the machine the release was **built**
on. On a user's machine it does not exist, so it is silently ignored — harmless,
but it is dead weight in the binary and it embeds the build layout in a shipped
artifact for no benefit.

Behind `#[cfg(debug_assertions)]`, where it is genuinely useful and where the
path is genuinely correct.

### 3. AppStream metainfo — missing

There is no `net.canfar.Verbinal.metainfo.xml`. That is what GNOME Software and
every other software centre read: without it the app has no summary, no
description, no screenshots and no release notes in any store listing, and
`appstreamcli validate` is a standard packaging gate.

The changelog already has the release notes; the metainfo's `<releases>` block
wants the same text, which is an argument for generating one from the other
rather than keeping two.

### 4. A guard, because four names must agree

The app id, the `.desktop` file's NAME, its `Icon=` line, and the icon
FILENAMES all have to be the string `net.canfar.Verbinal`. They are, today, by
hand. Nothing checks it, and the failure is exactly what is on screen now: a
generic icon, no error, no log line.

A test can read all four out of the tree — `src/lib.rs`, the desktop file,
`Cargo.toml`'s deb assets — and assert they match. Cheap, and it is the kind of
agreement that breaks during a rename when everything still compiles.

While there: assert every size the deb ships actually exists in `assets/`, so a
missing file is a failed test rather than a missing icon at one size on one
user's panel.

## What this does not cover

- **Redrawing the icon.** It is good, it matches the Windows app, and it is
  already a vector.
- **A symbolic variant** (`net.canfar.Verbinal-symbolic.svg`). GNOME Shell uses
  the full-colour icon; the symbolic one is for a shell that asks, and nothing
  asks yet.
- **Flatpak or Snap.** A separate packaging question with its own manifest, and
  the memory of this project says the `.deb` is the shipping path.
- **macOS or Windows assets.** Not targets of this repo.

## Order of work

1. The four-name guard, and the shipped-sizes-exist check. It is a test, it
   cannot break anything, and it pins what the rest assumes.
2. `CARGO_MANIFEST_DIR` behind `debug_assertions`.
3. The dev install/uninstall script.
4. The AppStream metainfo, validated in CI alongside the existing gates.

Step 1 is worth doing even if nothing else here is.
