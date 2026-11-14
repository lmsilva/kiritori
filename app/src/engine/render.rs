//! Graphics: the device, window capture, and drawing a surface's item list into its window's
//! swap chain. Picture items go through a small shader; chrome (shapes and text) through
//! Direct2D and DirectWrite on the same device.

use std::collections::HashMap;

use windows_numerics::{Matrix3x2, Vector2};
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::Fxc::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::WinRT::Direct3D11::*;
use windows::Win32::System::WinRT::Graphics::Capture::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::*;

use super::draw::{Align, Draw, Rgba};
use crate::model::geometry::Rect;
use crate::model::layout::{Filter, ShapeOp};

const FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
pub const MAX_RECTS: usize = 8;

const SHADER: &str = r#"
cbuffer C : register(b0) {
    float4 uv;            // texture coordinates of the region: u0, v0, u1, v1
    float4 bbox;          // region in source fractions: x, y, w, h
    float4 rects[8];      // shape rects in source fractions
    float4 ops[2];        // per rect: 1 add, -1 subtract, 0 unused
    float4 params;        // dim, alpha, circle clip, shape test on
    float4 orient;        // quarter turns (0-3), mirrored (0 or 1)
};
Texture2D tex : register(t0);
SamplerState smp : register(s0);
struct V { float4 pos : SV_Position; float2 q : TEXCOORD0; };
V vs(uint id : SV_VertexID) {
    float2 q = float2(id & 1, id >> 1);
    V o;
    o.pos = float4(q.x * 2 - 1, 1 - q.y * 2, 0, 1);
    o.q = q;
    return o;
}
float op(uint i) { return ops[i / 4][i % 4]; }
// A point on the placed cutout back to the same point of the unrotated shape (model::unorient).
float2 unorient(float2 q) {
    int t = (int)(orient.x + 0.5);
    if (t == 1) q = float2(q.y, 1 - q.x);
    else if (t == 2) q = float2(1 - q.x, 1 - q.y);
    else if (t == 3) q = float2(1 - q.y, q.x);
    if (orient.y > 0.5) q.x = 1 - q.x;
    return q;
}
float4 ps(V v) : SV_Target {
    V i = v;
    if (params.z > 0.5 && length(i.q - 0.5) > 0.5) discard;
    i.q = unorient(i.q);
    if (params.w > 0.5) {
        float2 f = bbox.xy + i.q * bbox.zw;
        bool inside = false;
        [unroll] for (uint k = 0; k < 8; k++) {
            float o = op(k);
            float4 r = rects[k];
            bool hit = f.x >= r.x && f.x < r.x + r.z && f.y >= r.y && f.y < r.y + r.w;
            if (o > 0.5) inside = inside || hit;
            else if (o < -0.5) inside = inside && !hit;
        }
        if (!inside) discard;
    }
    float3 c = tex.Sample(smp, lerp(uv.xy, uv.zw, i.q)).rgb * params.x;
    return float4(c, params.y);
}
"#;

#[repr(C)]
struct Constants {
    uv: [f32; 4],
    bbox: [f32; 4],
    rects: [[f32; 4]; MAX_RECTS],
    ops: [f32; MAX_RECTS],
    params: [f32; 4],
    orient: [f32; 4],
}

pub struct Gpu {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub ctx1: ID3D11DeviceContext1,
    factory: IDXGIFactory2,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    linear: ID3D11SamplerState,
    point: ID3D11SamplerState,
    blend: ID3D11BlendState,
    cbuf: ID3D11Buffer,
    wgc_device: IDirect3DDevice,
    d2d_factory: ID2D1Factory1,
    dc: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    dashed: ID2D1StrokeStyle,
    round: ID2D1StrokeStyle,
    icons: std::cell::RefCell<HashMap<&'static str, ID2D1PathGeometry>>,
    dwrite: IDWriteFactory,
    formats: std::cell::RefCell<HashMap<(u32, bool, u8), IDWriteTextFormat>>,
    /// Enhanced scaling, if its shaders compiled on this machine.
    fsr: Option<super::fsr::Fsr>,
    /// Cursor shapes turned into bitmaps, by cursor handle.
    cursors: std::cell::RefCell<HashMap<isize, Option<CursorImage>>>,
}

/// A cursor shape as a premultiplied bitmap, with its hot spot.
#[derive(Clone)]
pub struct CursorImage {
    bitmap: ID2D1Bitmap1,
    hot: (f32, f32),
    size: (f32, f32),
}

/// The shape the real cursor has now: over the app window, that is the app's own cursor.
fn current_cursor() -> Option<isize> {
    use windows::Win32::UI::WindowsAndMessaging::{CURSORINFO, GetCursorInfo};
    let mut ci = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
    unsafe { GetCursorInfo(&mut ci) }.ok()?;
    (!ci.hCursor.is_invalid()).then_some(ci.hCursor.0 as isize)
}

/// Draw a cursor on black and on white to recover its colours and transparency (this handles
/// colour, alpha and old monochrome cursors alike).
fn cursor_image(gpu: &Gpu, h: isize) -> Option<CursorImage> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let hc = HICON(h as *mut _);
        let mut ii = ICONINFO::default();
        GetIconInfo(hc, &mut ii).ok()?;
        let mut bm = BITMAP::default();
        let src = if !ii.hbmColor.is_invalid() { ii.hbmColor } else { ii.hbmMask };
        GetObjectW(src.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        let (w, hgt) = (bm.bmWidth, if ii.hbmColor.is_invalid() { bm.bmHeight / 2 } else { bm.bmHeight });
        let hot = (ii.xHotspot as f32, ii.yHotspot as f32);
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask.into());
        }
        if w <= 0 || hgt <= 0 || w > 256 || hgt > 256 {
            return None;
        }
        let render = |bg: u32| -> Option<Vec<u32>> {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -hgt, biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(dc, dib.into());
            let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * hgt) as usize);
            px.fill(bg);
            let _ = DrawIconEx(dc, 0, 0, hc, w, hgt, 0, None, DI_NORMAL);
            let out = px.to_vec();
            SelectObject(dc, old);
            let _ = DeleteObject(dib.into());
            let _ = DeleteDC(dc);
            Some(out)
        };
        let black = render(0xFF00_0000)?;
        let white = render(0xFFFF_FFFF)?;
        let mut pixels: Vec<u32> = Vec::with_capacity(black.len());
        for (b, wh) in black.iter().zip(white.iter()) {
            let a = 255 - (((wh >> 8) & 0xFF) as i32 - ((b >> 8) & 0xFF) as i32).clamp(0, 255) as u32;
            pixels.push((a << 24) | (b & 0x00FF_FFFF));
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: FORMAT, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap = gpu.dc.CreateBitmap(D2D_SIZE_U { width: w as u32, height: hgt as u32 }, Some(pixels.as_ptr() as *const _), (w * 4) as u32, &props).ok()?;
        Some(CursorImage { bitmap, hot, size: (w as f32, hgt as f32) })
    }
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    unsafe {
        let mut code = None;
        let mut err = None;
        let r = D3DCompile(SHADER.as_ptr() as *const _, SHADER.len(), s!("draw.hlsl"), None, None, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut code, Some(&mut err));
        if let Err(e) = r {
            if let Some(err) = err {
                let m = std::slice::from_raw_parts(err.GetBufferPointer() as *const u8, err.GetBufferSize());
                crate::log!("shader: {}", String::from_utf8_lossy(m));
            }
            return Err(e);
        }
        Ok(code.unwrap())
    }
}

fn bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

fn color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c[0], g: c[1], b: c[2], a: c[3] }
}

fn rect_f(r: (f32, f32, f32, f32)) -> D2D_RECT_F {
    D2D_RECT_F { left: r.0, top: r.1, right: r.0 + r.2, bottom: r.1 + r.3 }
}

impl Gpu {
    pub fn new() -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let device: ID3D11Device = device.unwrap();
            let ctx: ID3D11DeviceContext = ctx.unwrap();
            // Kiritori's few small draws go ahead of the game's work on the graphics card, so a
            // busy game never holds the copy back. Windows may refuse the highest values without
            // administrator rights; take the highest it allows.
            if let Ok(dxgi) = device.cast::<IDXGIDevice>() {
                for p in (1..=7).rev() {
                    if dxgi.SetGPUThreadPriority(p).is_ok() {
                        crate::log!("graphics priority {p}");
                        break;
                    }
                }
            }
            let (vsb, psb) = (compile(s!("vs"), s!("vs_5_0"))?, compile(s!("ps"), s!("ps_5_0"))?);
            let mut vs = None;
            device.CreateVertexShader(bytes(&vsb), None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(bytes(&psb), None, Some(&mut ps))?;
            let sampler = |filter| -> Result<ID3D11SamplerState> {
                let d = D3D11_SAMPLER_DESC {
                    Filter: filter,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                };
                let mut s = None;
                device.CreateSamplerState(&d, Some(&mut s))?;
                Ok(s.unwrap())
            };
            let mut bd = D3D11_BLEND_DESC::default();
            bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_SRC_ALPHA,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            device.CreateBlendState(&bd, Some(&mut blend))?;
            let cb = D3D11_BUFFER_DESC {
                ByteWidth: size_of::<Constants>().next_multiple_of(16) as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut cbuf = None;
            device.CreateBuffer(&cb, None, Some(&mut cbuf))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let wgc_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi)?;
            let dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = dc.CreateSolidColorBrush(&color([1.0; 4]), None)?;
            let props = D2D1_STROKE_STYLE_PROPERTIES1 { dashStyle: D2D1_DASH_STYLE_DASH, dashCap: D2D1_CAP_STYLE_FLAT, ..Default::default() };
            let dashed: ID2D1StrokeStyle = d2d_factory.CreateStrokeStyle(&props, None)?.cast()?;
            let rprops = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                ..Default::default()
            };
            let round: ID2D1StrokeStyle = d2d_factory.CreateStrokeStyle(&rprops, None)?.cast()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fsr = super::fsr::Fsr::new(&device);

            Ok(Gpu {
                ctx1: ctx.cast()?,
                factory: CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?,
                linear: sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR)?,
                point: sampler(D3D11_FILTER_MIN_MAG_MIP_POINT)?,
                blend: blend.unwrap(),
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                cbuf: cbuf.unwrap(),
                device,
                ctx,
                wgc_device,
                d2d_factory,
                dc,
                brush,
                dashed,
                round,
                icons: Default::default(),
                cursors: Default::default(),
                fsr,
                dwrite,
                formats: Default::default(),
            })
        }
    }

    /// An icon's outline as a Direct2D geometry on its 24×24 grid, built once and kept.
    fn icon(&self, name: &'static str) -> Result<ID2D1PathGeometry> {
        if let Some(g) = self.icons.borrow().get(name) {
            return Ok(g.clone());
        }
        use super::icons::{Seg, figures};
        let v = |p: (f32, f32)| Vector2 { X: p.0, Y: p.1 };
        let g = unsafe {
            let g: ID2D1PathGeometry = self.d2d_factory.CreatePathGeometry()?.cast()?;
            let sink = g.Open()?;
            for f in figures(name) {
                sink.BeginFigure(v(f.start), D2D1_FIGURE_BEGIN_HOLLOW);
                for s in &f.segs {
                    match *s {
                        Seg::Line(p) => sink.AddLine(v(p)),
                        Seg::Cubic(a, b, c) => sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: v(a), point2: v(b), point3: v(c) }),
                        Seg::Quad(a, b) => sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: v(a), point2: v(b) }),
                        Seg::Arc { to, r, rot, large, sweep } => sink.AddArc(&D2D1_ARC_SEGMENT {
                            point: v(to),
                            size: D2D_SIZE_F { width: r.0, height: r.1 },
                            rotationAngle: rot,
                            sweepDirection: if sweep { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                            arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                        }),
                    }
                }
                sink.EndFigure(if f.closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            }
            sink.Close()?;
            g
        };
        self.icons.borrow_mut().insert(name, g.clone());
        Ok(g)
    }

    /// The graphics card's name, Direct3D feature level and whether Enhanced scaling works.
    pub fn caps(&self) -> super::draw::EngineCaps {
        let adapter = (|| -> Result<String> {
            let dxgi: IDXGIDevice = self.device.cast()?;
            let desc = unsafe { dxgi.GetAdapter()?.GetDesc()? };
            Ok(String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string())
        })()
        .unwrap_or_else(|_| "unknown".into());
        let fl = unsafe { self.device.GetFeatureLevel() };
        let feature_level = match fl {
            D3D_FEATURE_LEVEL_11_1 => "11.1",
            D3D_FEATURE_LEVEL_11_0 => "11.0",
            _ => "other",
        }
        .to_string();
        super::draw::EngineCaps { adapter, feature_level, enhanced: self.fsr.is_some() }
    }

    fn format(&self, size: f32, bold: bool, align: Align) -> Result<IDWriteTextFormat> {
        let key = ((size * 10.0) as u32, bold, align as u8);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let f = unsafe {
            let f = self.dwrite.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f
        };
        self.formats.borrow_mut().insert(key, f.clone());
        Ok(f)
    }
}

// ---------------------------------------------------------------------------------------------
// Capture

/// Where the app's drawing area sits inside a captured frame, in pixels. Window capture
/// includes the title bar and frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub frame: (f64, f64),
    pub origin: (f64, f64),
    pub client: (f64, f64),
}

pub fn client_area(hwnd: HWND, frame: (u32, u32)) -> Area {
    unsafe {
        let mut bounds = RECT::default();
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut bounds as *mut _ as *mut _, size_of::<RECT>() as u32);
        let mut cr = RECT::default();
        let _ = GetClientRect(hwnd, &mut cr);
        let mut tl = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(hwnd, &mut tl);
        let (fw, fh) = (frame.0 as f64, frame.1 as f64);
        let ox = ((tl.x - bounds.left) as f64).clamp(0.0, fw);
        let oy = ((tl.y - bounds.top) as f64).clamp(0.0, fh);
        Area { frame: (fw, fh), origin: (ox, oy), client: ((cr.right as f64).min(fw - ox), (cr.bottom as f64).min(fh - oy)) }
    }
}

struct Copy {
    tex: ID3D11Texture2D,
    /// The whole mip chain, for cutouts drawn smaller than the source.
    view: ID3D11ShaderResourceView,
    /// The full-size level alone, when the smaller levels were not made for this frame.
    view0: ID3D11ShaderResourceView,
    size: (u32, u32),
    area: Area,
    mips: bool,
}

fn new_copy(gpu: &Gpu, size: (u32, u32), area: Area) -> Result<Copy> {
    let d = D3D11_TEXTURE2D_DESC {
        Width: size.0,
        Height: size.1,
        MipLevels: 0, // a full chain, so shrunk cutouts sample a pre-shrunk copy
        ArraySize: 1,
        Format: FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
        ..Default::default()
    };
    unsafe {
        let mut t = None;
        gpu.device.CreateTexture2D(&d, None, Some(&mut t))?;
        let tex = t.unwrap();
        let mut v = None;
        gpu.device.CreateShaderResourceView(&tex, None, Some(&mut v))?;
        let one = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: FORMAT,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
        };
        let mut v0 = None;
        gpu.device.CreateShaderResourceView(&tex, Some(&one), Some(&mut v0))?;
        Ok(Copy { tex, view: v.unwrap(), view0: v0.unwrap(), size, area, mips: false })
    }
}

pub struct Capture {
    pub hwnd: HWND,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: windows::Graphics::SizeInt32,
    live: Option<Copy>,
    still: Option<Copy>,
    pub frames: u32,
    /// When the newest frame arrived.
    pub arrived: Option<std::time::Instant>,
    arrival_token: Option<i64>,
}

impl Capture {
    /// Start capturing `hwnd`. `arrived` is signalled the moment each new frame is ready, so the
    /// engine draws it straight away instead of at its next look.
    pub fn start(gpu: &Gpu, hwnd: HWND, arrived: HANDLE) -> Result<Self> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.wgc_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
        let event = arrived.0 as isize;
        let token = pool
            .FrameArrived(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                unsafe {
                    let _ = windows::Win32::System::Threading::SetEvent(HANDLE(event as *mut _));
                }
                Ok(())
            }))
            .ok();
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        // Every frame the app draws, not one per screen refresh (Windows 11 24H2 and later).
        let _ = session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 10_000 });
        session.StartCapture()?;
        Ok(Capture { hwnd, pool, session, size, live: None, still: None, frames: 0, arrived: None, arrival_token: token })
    }

    /// Copy the newest frame into the live texture, if one arrived; true if one did. `mips`:
    /// also shrink it for cutouts drawn smaller than the source (skipped otherwise: on a large
    /// source it is most of the copy's cost).
    pub fn update(&mut self, gpu: &Gpu, mips: bool) -> Result<bool> {
        let mut latest = None;
        while let Ok(f) = self.pool.TryGetNextFrame() {
            latest = Some(f);
        }
        let Some(frame) = latest else { return Ok(false) };
        let content = frame.ContentSize()?;
        if content.Width != self.size.Width || content.Height != self.size.Height {
            self.size = content;
            self.pool.Recreate(&gpu.wgc_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
            return Ok(false);
        }
        let size = (content.Width.max(1) as u32, content.Height.max(1) as u32);
        let area = client_area(self.hwnd, size);
        unsafe {
            let tex: ID3D11Texture2D = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()?;
            if self.live.as_ref().is_none_or(|c| c.size != size) {
                self.live = Some(new_copy(gpu, size, area)?);
            }
            let live = self.live.as_mut().unwrap();
            live.area = area;
            let b = D3D11_BOX { left: 0, top: 0, front: 0, right: size.0, bottom: size.1, back: 1 };
            gpu.ctx.CopySubresourceRegion(&live.tex, 0, 0, 0, 0, &tex, 0, Some(&b));
            if mips {
                gpu.ctx.GenerateMips(&live.view);
            }
            live.mips = mips;
        }
        self.frames += 1;
        self.arrived = Some(std::time::Instant::now());
        Ok(true)
    }

    /// Keep the current live frame as the still (Cut view, Retake).
    pub fn freeze(&mut self, gpu: &Gpu) -> Result<()> {
        let Some(live) = &self.live else { return Ok(()) };
        if self.still.as_ref().is_none_or(|s| s.size != live.size) {
            self.still = Some(new_copy(gpu, live.size, live.area)?);
        }
        let still = self.still.as_mut().unwrap();
        still.area = live.area;
        unsafe {
            gpu.ctx.CopyResource(&still.tex, &live.tex);
            // The still is drawn at any size (Cut view, other displays): give it every level.
            if !live.mips {
                gpu.ctx.GenerateMips(&still.view);
            }
        }
        still.mips = true;
        Ok(())
    }

    /// The drawing area's pixels (BGRA, top row first) of the still frame, or of the live
    /// picture if there is no still, for Suggest and Trim.
    pub fn snapshot(&self, gpu: &Gpu) -> Result<super::draw::Snapshot> {
        let c = self.still.as_ref().or(self.live.as_ref()).ok_or_else(|| Error::new(E_FAIL, "no picture yet"))?;
        let (ox, oy) = (c.area.origin.0.max(0.0) as u32, c.area.origin.1.max(0.0) as u32);
        let w = (c.area.client.0 as u32).min(c.size.0.saturating_sub(ox)).max(1);
        let h = (c.area.client.1 as u32).min(c.size.1.saturating_sub(oy)).max(1);
        unsafe {
            let d = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: FORMAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            gpu.device.CreateTexture2D(&d, None, Some(&mut staging))?;
            let staging = staging.unwrap();
            let b = D3D11_BOX { left: ox, top: oy, front: 0, right: ox + w, bottom: oy + h, back: 1 };
            gpu.ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &c.tex, 0, Some(&b));
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let mut bgra = Vec::with_capacity((w * h * 4) as usize);
            for row in 0..h {
                let p = (m.pData as *const u8).add((row * m.RowPitch) as usize);
                bgra.extend_from_slice(std::slice::from_raw_parts(p, (w * 4) as usize));
            }
            gpu.ctx.Unmap(&staging, 0);
            Ok(super::draw::Snapshot { w: w as usize, h: h as usize, bgra })
        }
    }

    /// The live texture's size, once a frame has arrived.
    pub fn source_size(&self) -> Option<(u32, u32)> {
        self.live.as_ref().map(|c| c.size)
    }

    pub fn source(&self, frozen: bool) -> Option<(&ID3D11ShaderResourceView, Area)> {
        let c = if frozen { self.still.as_ref().or(self.live.as_ref()) } else { self.live.as_ref() };
        c.map(|c| (if c.mips { &c.view } else { &c.view0 }, c.area))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(t) = self.arrival_token {
            let _ = self.pool.RemoveFrameArrived(t);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// GPU time for drawing one canvas, with timestamp queries read back a frame or two later so
/// nothing waits for the graphics card.
pub struct GpuTimer {
    disjoint: ID3D11Query,
    start: ID3D11Query,
    end: ID3D11Query,
    pending: bool,
    pub last_ms: Option<f32>,
}

impl GpuTimer {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let q = |kind| -> Result<ID3D11Query> {
            let mut out = None;
            unsafe { gpu.device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut out))? };
            Ok(out.unwrap())
        };
        Ok(GpuTimer { disjoint: q(D3D11_QUERY_TIMESTAMP_DISJOINT)?, start: q(D3D11_QUERY_TIMESTAMP)?, end: q(D3D11_QUERY_TIMESTAMP)?, pending: false, last_ms: None })
    }

    /// Start timing, unless the last measurement has not come back yet.
    pub fn begin(&mut self, gpu: &Gpu) -> bool {
        self.collect(gpu);
        if self.pending {
            return false;
        }
        unsafe {
            gpu.ctx.Begin(&self.disjoint);
            gpu.ctx.End(&self.start);
        }
        true
    }

    pub fn end(&mut self, gpu: &Gpu) {
        unsafe {
            gpu.ctx.End(&self.end);
            gpu.ctx.End(&self.disjoint);
        }
        self.pending = true;
    }

    fn collect(&mut self, gpu: &Gpu) {
        if !self.pending {
            return;
        }
        unsafe {
            let mut dj = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
            // Not ready yet (S_FALSE) leaves the data untouched: the frequency stays 0.
            if gpu.ctx.GetData(&self.disjoint, Some(&mut dj as *mut _ as *mut _), size_of::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>() as u32, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32).is_err() || dj.Frequency == 0 {
                return;
            }
            let (mut a, mut b) = (0u64, 0u64);
            let _ = gpu.ctx.GetData(&self.start, Some(&mut a as *mut _ as *mut _), 8, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32);
            let _ = gpu.ctx.GetData(&self.end, Some(&mut b as *mut _ as *mut _), 8, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32);
            self.pending = false;
            if !dj.Disjoint.as_bool() && dj.Frequency > 0 && b > a {
                self.last_ms = Some((b - a) as f32 * 1000.0 / dj.Frequency as f32);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Swap chain on a canvas window owned by the shell.

pub struct Target {
    pub hwnd: isize,
    pub size: (u32, u32),
    swap: IDXGISwapChain2,
    pub waitable: HANDLE,
    pub transparent: bool,
    /// Presents go to the screen at once, without waiting for the next refresh (a tear line
    /// can show). Only for opaque canvases, and only when Windows supports it.
    tearing: bool,
    /// DirectComposition objects that show a transparent swap chain in its window.
    _comp: Option<(IDCompositionDevice, IDCompositionTarget, IDCompositionVisual)>,
}

/// Whether Windows and the graphics driver can present without waiting for a refresh.
fn tearing_supported(gpu: &Gpu) -> bool {
    let Ok(f5) = gpu.factory.cast::<IDXGIFactory5>() else { return false };
    let mut allow = BOOL(0);
    unsafe { f5.CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &mut allow as *mut _ as *mut _, size_of::<BOOL>() as u32) }.is_ok() && allow.as_bool()
}

impl Target {
    pub fn new(gpu: &Gpu, hwnd: isize, size: (u32, u32), transparent: bool) -> Result<Self> {
        let tearing = !transparent && std::env::var("KIRITORI_ALLOW_TEARING").is_ok_and(|v| v == "1") && tearing_supported(gpu);
        unsafe {
            let mut desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: FORMAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Flags: (DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 | if tearing { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 } else { 0 }) as u32,
                ..Default::default()
            };
            let (swap, comp): (IDXGISwapChain2, _) = if transparent {
                // Float: a premultiplied-alpha swap chain shown through DirectComposition, so the
                // desktop shows wherever nothing is drawn.
                desc.AlphaMode = DXGI_ALPHA_MODE_PREMULTIPLIED;
                desc.Scaling = DXGI_SCALING_STRETCH;
                let swap: IDXGISwapChain2 = gpu.factory.CreateSwapChainForComposition(&gpu.device, &desc, None)?.cast()?;
                let dxgi: IDXGIDevice = gpu.device.cast()?;
                let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
                let target = dcomp.CreateTargetForHwnd(HWND(hwnd as *mut _), true)?;
                let visual = dcomp.CreateVisual()?;
                visual.SetContent(&swap)?;
                target.SetRoot(&visual)?;
                dcomp.Commit()?;
                (swap, Some((dcomp, target, visual)))
            } else {
                (gpu.factory.CreateSwapChainForHwnd(&gpu.device, HWND(hwnd as *mut _), &desc, None, None)?.cast()?, None)
            };
            swap.SetMaximumFrameLatency(1)?;
            let waitable = swap.GetFrameLatencyWaitableObject();
            Ok(Target { hwnd, size, swap, waitable, transparent, tearing, _comp: comp })
        }
    }

    pub fn render(&mut self, gpu: &Gpu, draws: &[Draw], src: Option<(&ID3D11ShaderResourceView, Area)>, cursor: Option<(i32, i32)>) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            let mut rtv = None;
            gpu.device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            let rtv = rtv.unwrap();
            gpu.ctx.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, if self.transparent { 0.0 } else { 1.0 }]);
            let surface: IDXGISurface = back.cast()?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: FORMAT, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bitmap = gpu.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            gpu.dc.SetTarget(&bitmap);
            gpu.dc.SetTransform(&Matrix3x2::identity());

            let mut in_2d = false;
            for d in draws {
                let is_source = matches!(d, Draw::Source { .. });
                if is_source && in_2d {
                    gpu.dc.EndDraw(None, None)?;
                    in_2d = false;
                }
                if !is_source && !in_2d {
                    gpu.dc.BeginDraw();
                    in_2d = true;
                }
                match d {
                    Draw::Source { orient, dst, src: region, shape, filter, dim, alpha, circle, .. } => {
                        if let Some((view, area)) = src {
                            draw_source(gpu, &rtv, view, area, *dst, *region, shape, *filter, *dim, *alpha, *circle, *orient);
                        }
                    }
                    Draw::Fill { r, color: c, radius } => {
                        gpu.brush.SetColor(&color(*c));
                        let rr = D2D1_ROUNDED_RECT { rect: rect_f(*r), radiusX: *radius, radiusY: *radius };
                        gpu.dc.FillRoundedRectangle(&rr, &gpu.brush);
                    }
                    Draw::Stroke { r, color: c, width, radius, dashed } => {
                        gpu.brush.SetColor(&color(*c));
                        let rr = D2D1_ROUNDED_RECT { rect: rect_f(*r), radiusX: *radius, radiusY: *radius };
                        let style = if *dashed { Some(&gpu.dashed) } else { None };
                        gpu.dc.DrawRoundedRectangle(&rr, &gpu.brush, *width, style);
                    }
                    Draw::Line { a, b, color: c, width } => {
                        gpu.brush.SetColor(&color(*c));
                        gpu.dc.DrawLine(Vector2 { X: a.0, Y: a.1 }, Vector2 { X: b.0, Y: b.1 }, &gpu.brush, *width, None);
                    }
                    Draw::Ellipse { r, color: c, width } => {
                        gpu.brush.SetColor(&color(*c));
                        let e = D2D1_ELLIPSE { point: Vector2 { X: r.0 + r.2 / 2.0, Y: r.1 + r.3 / 2.0 }, radiusX: r.2 / 2.0, radiusY: r.3 / 2.0 };
                        gpu.dc.DrawEllipse(&e, &gpu.brush, *width, None);
                    }
                    Draw::Icon { name, r, color: c, stroke } => {
                        if let Ok(g) = gpu.icon(name) {
                            gpu.brush.SetColor(&color(*c));
                            let s = r.2 / 24.0;
                            gpu.dc.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: r.0, M32: r.1 });
                            gpu.dc.DrawGeometry(&g, &gpu.brush, stroke / s, Some(&gpu.round));
                            gpu.dc.SetTransform(&Matrix3x2::identity());
                        }
                    }
                    Draw::Text { r, text, size, color: c, bold, align } => {
                        gpu.brush.SetColor(&color(*c));
                        let f = gpu.format(*size, *bold, *align)?;
                        let w: Vec<u16> = text.encode_utf16().collect();
                        gpu.dc.DrawText(&w, &f, &rect_f(*r), &gpu.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
            }
            if in_2d {
                gpu.dc.EndDraw(None, None)?;
            }
            // The pointer, in the app's own cursor shape.
            let image = cursor.and_then(|_| current_cursor()).and_then(|h| gpu.cursors.borrow_mut().entry(h).or_insert_with(|| cursor_image(gpu, h)).clone());
            match (cursor, image) {
                (Some((x, y)), Some(img)) => {
                    gpu.dc.BeginDraw();
                    let r = D2D_RECT_F { left: x as f32 - img.hot.0, top: y as f32 - img.hot.1, right: x as f32 - img.hot.0 + img.size.0, bottom: y as f32 - img.hot.1 + img.size.1 };
                    gpu.dc.DrawBitmap(&img.bitmap, Some(&r), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
                    gpu.dc.EndDraw(None, None)?;
                    gpu.dc.SetTarget(None);
                }
                (Some((x, y)), None) => {
                    gpu.dc.SetTarget(None);
                    draw_cursor(gpu, &rtv, x, y);
                }
                _ => gpu.dc.SetTarget(None),
            }
            drop(bitmap);
            drop(rtv);
            drop(back);
            // Hand the frame over now; the waitable object (latency 1) already paces drawing to
            // what the screen can show, so waiting for the next refresh here only adds lag.
            self.swap.Present(0, if self.tearing { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) }).ok()
        }
    }
}


#[allow(clippy::too_many_arguments)]
fn draw_source(
    gpu: &Gpu,
    rtv: &ID3D11RenderTargetView,
    view: &ID3D11ShaderResourceView,
    a: Area,
    dst: (f32, f32, f32, f32),
    region: Rect,
    shape: &[crate::model::layout::ShapeRect],
    filter: Filter,
    dim: f32,
    alpha: f32,
    circle: bool,
    orient: u8,
) {
    let (x, y, w, h) = dst;
    if w < 1.0 || h < 1.0 {
        return;
    }
    // Auto: Enhanced when enlarged by more than 10%, otherwise Smooth.
    let src_w = if orient & 1 == 1 { region.h * a.client.1 } else { region.w * a.client.0 };
    let enlarged = w as f64 / src_w.max(1.0);
    let filter = match filter {
        Filter::Auto if enlarged > 1.1 => Filter::Enhanced,
        Filter::Auto => Filter::Smooth,
        f => f,
    };
    // Enhanced works on the picture as it is; a turned or mirrored cutout is drawn Smooth.
    if filter == Filter::Enhanced && !circle && orient == 0 && enlarged > 1.0 && let Some(fsr) = &gpu.fsr
        && draw_enhanced(gpu, fsr, rtv, view, a, dst, region, shape, dim, alpha).is_ok() {
            return;
        }
    unsafe {
        gpu.ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        gpu.ctx.OMSetBlendState(&gpu.blend, None, 0xFFFF_FFFF);
        gpu.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        gpu.ctx.VSSetShader(&gpu.vs, None);
        gpu.ctx.PSSetShader(&gpu.ps, None);
        gpu.ctx.PSSetConstantBuffers(0, Some(&[Some(gpu.cbuf.clone())]));
        gpu.ctx.PSSetShaderResources(0, Some(&[Some(view.clone())]));
        gpu.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: x, TopLeftY: y, Width: w, Height: h, MinDepth: 0.0, MaxDepth: 1.0 }]));
        let u = |f: f64| ((a.origin.0 + f * a.client.0) / a.frame.0) as f32;
        let v = |f: f64| ((a.origin.1 + f * a.client.1) / a.frame.1) as f32;
        let b = region;
        let mut c = Constants {
            uv: [u(b.x), v(b.y), u(b.right()), v(b.bottom())],
            bbox: [b.x as f32, b.y as f32, b.w as f32, b.h as f32],
            rects: [[0.0; 4]; MAX_RECTS],
            ops: [0.0; MAX_RECTS],
            params: [dim, alpha, if circle { 1.0 } else { 0.0 }, if shape.is_empty() { 0.0 } else { 1.0 }],
            orient: [(orient & 3) as f32, if orient & 4 != 0 { 1.0 } else { 0.0 }, 0.0, 0.0],
        };
        for (k, r) in shape.iter().take(MAX_RECTS).enumerate() {
            c.rects[k] = [r.rect.x as f32, r.rect.y as f32, r.rect.w as f32, r.rect.h as f32];
            c.ops[k] = if r.op == ShapeOp::Add { 1.0 } else { -1.0 };
        }
        gpu.ctx.UpdateSubresource(&gpu.cbuf, 0, None, &c as *const _ as *const _, 0, 0);
        let s = if filter == Filter::Sharp { &gpu.point } else { &gpu.linear };
        gpu.ctx.PSSetSamplers(0, Some(&[Some(s.clone())]));
        gpu.ctx.Draw(4, 0);
        gpu.ctx.OMSetRenderTargets(None, None);
    }
}

/// Constants for the shape test and dimming, shared by the plain and the Enhanced paths.
fn shape_constants(a: Area, region: Rect, shape: &[crate::model::layout::ShapeRect], dim: f32, alpha: f32, circle: bool) -> Constants {
    let u = |f: f64| ((a.origin.0 + f * a.client.0) / a.frame.0) as f32;
    let v = |f: f64| ((a.origin.1 + f * a.client.1) / a.frame.1) as f32;
    let b = region;
    let mut c = Constants {
        uv: [u(b.x), v(b.y), u(b.right()), v(b.bottom())],
        bbox: [b.x as f32, b.y as f32, b.w as f32, b.h as f32],
        rects: [[0.0; 4]; MAX_RECTS],
        ops: [0.0; MAX_RECTS],
        params: [dim, alpha, if circle { 1.0 } else { 0.0 }, if shape.is_empty() { 0.0 } else { 1.0 }],
        orient: [0.0; 4],
    };
    for (k, r) in shape.iter().take(MAX_RECTS).enumerate() {
        c.rects[k] = [r.rect.x as f32, r.rect.y as f32, r.rect.w as f32, r.rect.h as f32];
        c.ops[k] = if r.op == ShapeOp::Add { 1.0 } else { -1.0 };
    }
    c
}

/// FSR 1: EASU upscales the region into a temporary image at the output size, then RCAS
/// sharpens it onto the canvas with the cutout's shape.
#[allow(clippy::too_many_arguments)]
fn draw_enhanced(
    gpu: &Gpu,
    fsr: &super::fsr::Fsr,
    rtv: &ID3D11RenderTargetView,
    view: &ID3D11ShaderResourceView,
    a: Area,
    dst: (f32, f32, f32, f32),
    region: Rect,
    shape: &[crate::model::layout::ShapeRect],
    dim: f32,
    alpha: f32,
) -> Result<()> {
    let (x, y) = (dst.0.round(), dst.1.round());
    let (ow, oh) = (dst.2.round().max(1.0) as u32, dst.3.round().max(1.0) as u32);
    let mid = fsr.mid(&gpu.device, (ow, oh))?;
    let fc = super::fsr::FsrConstants {
        easu_in: [(region.w * a.client.0) as f32, (region.h * a.client.1) as f32, a.frame.0 as f32, a.frame.1 as f32],
        easu_out: [ow as f32, oh as f32, (a.origin.0 + region.x * a.client.0) as f32, (a.origin.1 + region.y * a.client.1) as f32],
        rcas_p: [super::fsr::SHARPNESS_STOPS, x, y, 0.0],
        rcas_q: [ow as f32, oh as f32, 0.0, 0.0],
    };
    let c = shape_constants(a, region, shape, dim, alpha, false);
    unsafe {
        gpu.ctx.UpdateSubresource(&fsr.cbuf, 0, None, &fc as *const _ as *const _, 0, 0);
        gpu.ctx.UpdateSubresource(&gpu.cbuf, 0, None, &c as *const _ as *const _, 0, 0);
        gpu.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        gpu.ctx.VSSetShader(&gpu.vs, None);
        gpu.ctx.PSSetConstantBuffers(0, Some(&[Some(gpu.cbuf.clone()), Some(fsr.cbuf.clone())]));
        // Pass 1: EASU into the temporary image.
        gpu.ctx.OMSetBlendState(None, None, 0xFFFF_FFFF);
        gpu.ctx.OMSetRenderTargets(Some(&[Some(mid.rtv.clone())]), None);
        gpu.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: ow as f32, Height: oh as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
        gpu.ctx.PSSetShader(&fsr.easu, None);
        gpu.ctx.PSSetShaderResources(0, Some(&[Some(view.clone())]));
        gpu.ctx.PSSetSamplers(0, Some(&[Some(gpu.linear.clone())]));
        gpu.ctx.Draw(4, 0);
        gpu.ctx.OMSetRenderTargets(None, None);
        // Pass 2: RCAS onto the canvas.
        gpu.ctx.OMSetBlendState(&gpu.blend, None, 0xFFFF_FFFF);
        gpu.ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        gpu.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: x, TopLeftY: y, Width: ow as f32, Height: oh as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
        gpu.ctx.PSSetShader(&fsr.rcas, None);
        gpu.ctx.PSSetShaderResources(0, Some(&[Some(mid.srv.clone())]));
        gpu.ctx.Draw(4, 0);
        gpu.ctx.PSSetShaderResources(0, Some(&[None]));
        gpu.ctx.OMSetRenderTargets(None, None);
    }
    Ok(())
}

/// Kiritori's own pointer while the real one is parked over the app: a white arrow with a
/// black edge, built from small rectangles.
fn draw_cursor(gpu: &Gpu, rtv: &ID3D11RenderTargetView, x: i32, y: i32) {
    let s = (unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as i32 / 96).max(1);
    let mut black = Vec::new();
    let mut white = Vec::new();
    for row in 0..17 {
        let width = (if row < 12 { row + 1 } else { 17 - row - 1 }).max(1);
        black.push(RECT { left: x - s, top: y + row * s - s, right: x + (width + 1) * s, bottom: y + (row + 1) * s + s / 2 });
        if row > 0 && row < 16 {
            white.push(RECT { left: x, top: y + row * s, right: x + (width - 1).max(1) * s, bottom: y + (row + 1) * s });
        }
    }
    unsafe {
        gpu.ctx1.ClearView(rtv, &[0.0, 0.0, 0.0, 1.0], Some(&black));
        gpu.ctx1.ClearView(rtv, &[1.0, 1.0, 1.0, 1.0], Some(&white));
    }
}

/// Text width in pixels, for sizing tags and pills. Usable from any thread.
pub fn measure(text: &str, size: f32, bold: bool) -> f32 {
    thread_local! {
        static DW: Option<IDWriteFactory> = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok() };
    }
    DW.with(|dw| {
        let Some(dw) = dw else { return text.chars().count() as f32 * size * 0.55 };
        unsafe {
            let Ok(f) = dw.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            ) else {
                return text.chars().count() as f32 * size * 0.55;
            };
            let w: Vec<u16> = text.encode_utf16().collect();
            let Ok(layout) = dw.CreateTextLayout(&w, &f, 10000.0, 1000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut m);
            m.widthIncludingTrailingWhitespace
        }
    })
}
