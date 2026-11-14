# Kiritori stand-in

A fake four-quadrant "game" for testing Kiritori without a real one.

It draws a 2×2 picture and animates every quadrant: a white block sweeps across each one, and the yellow cells near each quadrant's top-left count frames in binary. If either stops moving, the picture is frozen. Every click is marked with a crosshair and logged at the exact source pixel it lands on. A crosshair turns red while the button is held.

## Run

```
cargo run -p kiritori-standin -- --size 1920x1080 --mode framed
```

| Option | Meaning | Default |
|---|---|---|
| `--size WxH` | Render size, the size of the "game" picture | `1920x1080` |
| `--mode` | `framed` (normal window with title bar), `borderless`, or `fullscreen` (exclusive) | `framed` |
| `--image path` | Use an image (PNG, JPEG, WebP), stretched to the render size, instead of the test pattern | test pattern |
| `--display N` | Display to open on, numbered in Windows' enumeration order | the window's current display |
| `--title text` | Window title prefix, so several copies can be told apart | `Kiritori stand-in` |
| `--plain` | Draw the picture alone, without the moving blocks and frame counters (for screenshots) | off |
| `--timing file` | Write one line per frame, `frame qpc`, for latency measurements | off |

To test against an elevated app, start it from an administrator terminal. The title bar shows `elevated: yes`.

## Keys (while the stand-in is focused)

| Key | Action |
|---|---|
| 1 | Framed window |
| 2 | Borderless window |
| 3 | Exclusive full screen |
| C | Clear click markers |
| Ctrl+Q | Quit |

## Test pattern

Four tinted quadrants with a 40 px grid, white 10 px ticks along each quadrant's top and left edges, a white quadrant outline, and a 1 px checkerboard patch at each quadrant's bottom-left. The checkerboard turns grey when a scaler blurs it, which makes it easy to compare upscaling methods.

## Log

Each event is written to `kiritori-standin.log` next to the executable and to stderr:

```
    57.397  left   down source 700,400 BR  client 700,400  via mouse
```

`source` is the pixel in the render-size picture and `client` is the window point that received it; they differ when the window is stretched. `via` is `mouse`, `pen` or `touch`, from the signature Windows puts on mouse messages it synthesised from touch or pen.
