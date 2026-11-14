//! Kiritori's added lag, measured end to end.
//!
//! The stand-in (`kiritori-standin --timing file`) draws a 16-bit frame counter in each
//! quadrant and logs when it presented each frame. This tool watches the display that shows
//! Kiritori's copy of that counter through Desktop Duplication, reads the frame number off the
//! screen, and takes the time that image reached the desktop. The difference is how long a frame
//! the app presented takes to appear in Kiritori's output.
//!
//! ```text
//! kiritori-latency --at X,Y --cell X,Y --step S --seconds N --timing file
//! ```
//! `--at`: any screen point on the output display. `--cell`: where, on that display (relative
//! to its top-left), the centre of the counter's highest bit is drawn; `--step`: the distance
//! between bit cells there, in pixels.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Performance::*;
use windows::Win32::UI::HiDpi::*;
use windows::core::*;

struct Args {
    at: (i32, i32),
    cell: (i32, i32),
    step: i32,
    seconds: f64,
    timing: String,
}

fn pair(s: &str) -> std::result::Result<(i32, i32), String> {
    let (a, b) = s.split_once(',').ok_or("expected X,Y")?;
    Ok((a.trim().parse().map_err(|_| "bad number")?, b.trim().parse().map_err(|_| "bad number")?))
}

fn args() -> std::result::Result<Args, String> {
    let mut a = Args { at: (0, 0), cell: (50, 50), step: 28, seconds: 10.0, timing: String::new() };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().ok_or(format!("{k} needs a value"))?;
        match k.as_str() {
            "--at" => a.at = pair(&v)?,
            "--cell" => a.cell = pair(&v)?,
            "--step" => a.step = v.parse().map_err(|_| "bad step")?,
            "--seconds" => a.seconds = v.parse().map_err(|_| "bad seconds")?,
            "--timing" => a.timing = v,
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    if a.timing.is_empty() {
        return Err("--timing file is required".into());
    }
    Ok(a)
}

/// The adapter and output that show screen point `at`.
fn find_output(at: (i32, i32)) -> Result<(IDXGIAdapter1, IDXGIOutput1, RECT)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                let d = output.GetDesc()?;
                let r = d.DesktopCoordinates;
                if at.0 >= r.left && at.0 < r.right && at.1 >= r.top && at.1 < r.bottom {
                    return Ok((adapter, output.cast()?, r));
                }
                j += 1;
            }
            i += 1;
        }
        Err(Error::new(E_FAIL, "no display at that point"))
    }
}

fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&a) {
        eprintln!("failed: {e}");
        std::process::exit(1);
    }
}

fn run(a: &Args) -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let (adapter, output, rect) = find_output(a.at)?;
        let mut device = None;
        let mut ctx = None;
        D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))?;
        let (device, ctx) = (device.unwrap(), ctx.unwrap());
        let dup = output.DuplicateOutput(&device)?;

        // A strip covering the 16 cells, copied off the GPU each frame.
        let (x0, y0) = (a.cell.0 - a.step / 2, a.cell.1 - 2);
        let (sw, sh) = ((16 * a.step) as u32, 5u32);
        let sd = D3D11_TEXTURE2D_DESC {
            Width: sw,
            Height: sh,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..Default::default()
        };
        let mut staging = None;
        device.CreateTexture2D(&sd, None, Some(&mut staging))?;
        let staging = staging.unwrap();

        let mut freq = 0i64;
        QueryPerformanceFrequency(&mut freq)?;
        let start = std::time::Instant::now();
        let mut seen: Vec<(u32, i64)> = Vec::new();
        let mut last = u32::MAX;
        let (mut updates, mut first) = (0u32, Vec::new());
        println!("watching {}x{} at {},{}", rect.right - rect.left, rect.bottom - rect.top, rect.left, rect.top);
        while start.elapsed().as_secs_f64() < a.seconds {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res = None;
            match dup.AcquireNextFrame(100, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(e) => return Err(e),
            }
            if info.LastPresentTime != 0
                && let Some(r) = res
            {
                let tex: ID3D11Texture2D = r.cast()?;
                let b = D3D11_BOX { left: x0 as u32, top: y0 as u32, front: 0, right: x0 as u32 + sw, bottom: y0 as u32 + sh, back: 1 };
                ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &tex, 0, Some(&b));
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                let row = (m.pData as *const u8).add(2 * m.RowPitch as usize);
                let mut n = 0u32;
                for bit in 0..16 {
                    // Bit 15 is leftmost. Lit cells are yellow: red and green high, blue low.
                    let x = (a.step / 2 + (15 - bit) * a.step) as usize;
                    let p = std::slice::from_raw_parts(row.add(x * 4), 4);
                    if p[2] > 160 && p[1] > 160 && p[0] < 120 {
                        n |= 1 << bit;
                    }
                }
                ctx.Unmap(&staging, 0);
                updates += 1;
                if first.len() < 5 {
                    first.push(n);
                }
                if n != last {
                    seen.push((n, info.LastPresentTime));
                    last = n;
                }
            }
            let _ = dup.ReleaseFrame();
        }

        // Match each number seen on screen with when the stand-in presented it.
        let text = std::fs::read_to_string(&a.timing).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
        let mut presented = std::collections::HashMap::new();
        for line in text.lines() {
            if let Some((f, q)) = line.split_once(' ')
                && let (Ok(f), Ok(q)) = (f.parse::<u32>(), q.parse::<i64>())
            {
                presented.insert(f, q);
            }
        }
        let mut ms: Vec<f64> = seen
            .iter()
            .filter_map(|(n, shown)| presented.get(n).map(|p| (shown - p) as f64 * 1000.0 / freq as f64))
            .filter(|d| (0.0..500.0).contains(d))
            .collect();
        if ms.is_empty() {
            println!("no frames matched ({} numbers read, {} screen updates, first {:?}, {} frames logged)", seen.len(), updates, first, presented.len());
            return Ok(());
        }
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        let mean = ms.iter().sum::<f64>() / ms.len() as f64;
        println!(
            "frames {} (of {} read) · lag ms: min {:.1} · median {:.1} · mean {:.1} · p90 {:.1} · max {:.1}",
            ms.len(),
            seen.len(),
            ms[0],
            pct(0.5),
            mean,
            pct(0.9),
            ms[ms.len() - 1]
        );
        Ok(())
    }
}
