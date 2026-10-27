# The application icon

The application icon of Junto: three overlapping discs in terracotta, ochre and cypress, whose shared area is light, on the limestone background `#E3D7C1` (ADR 0756).

| File | Content | Use |
|---|---|---|
| `junto.svg` | The drawing in a rounded square, with the margin and the shadow of the macOS icon grid, 1024 by 1024. | The source of every icon of the packages on macOS, Windows and Linux. |
| `junto-square.svg` | The same drawing as a full square without transparency. | A place that needs a square, such as a web page or a store listing. No package uses it. |

No raster picture of the icon is in the repository. The crate `junto-icon` (`crates/icon`) renders `junto.svg` with resvg at every build: the packaging scripts take its program `junto-icon`, and the build script of the client uses the crate for the icon of the Windows program and of the X11 windows. To write one file:

```sh
cargo run -p junto-icon -- png 256 /absolute/new/junto-256.png
cargo run -p junto-icon -- icns /absolute/new/Junto.icns
cargo run -p junto-icon -- ico /absolute/new/junto.ico
```

## Tray pictures

`tray/` holds the signs of the tray pictures (ADR 0759): `junto.svg`, three rings in the places of the discs of the icon, outside a call; `microphone.svg`, `microphone-off.svg`, `microphone-held.svg` and `headphones-off.svg` in a call. Each is 24 by 24 with its strokes in `currentColor`. The crate `junto-icon` draws them black on transparent for the template pictures of the macOS menu bar, and white on a disc of the colour of the call state for Windows and Linux (`junto_icon::tray`). The four signs of a call are the Lucide drawings that the window uses, with a wider stroke; their licence is `tray/LICENSE-lucide`.

## Licence

The icon is the project's own work, under the licence of the repository, Apache-2.0 (`LICENSE`).
