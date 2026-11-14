//! Enhanced scaling: AMD FidelityFX Super Resolution 1 (MIT licence, see
//! assets/fsr1/license.txt), pinned to v1.0.2.
//!
//! Two passes per enlarged cutout: EASU (edge-adaptive spatial upsampling) draws the cutout's
//! source region into a temporary image at its on-screen size; RCAS (robust contrast-adaptive
//! sharpening) draws that onto the canvas, applying the cutout's add/subtract shape. FSR 1 only
//! needs finished frames and runs on any Direct3D 11 graphics card.
//!
//! FSR's constant setup is written to run on the CPU or the GPU; it runs in the shader here, so
//! nothing had to be ported.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Graphics::Direct3D::Fxc::*;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::*;

const FFX_A: &str = include_str!("../../assets/fsr1/ffx_a.h");
const FFX_FSR1: &str = include_str!("../../assets/fsr1/ffx_fsr1.h");

/// How much RCAS sharpens, in stops below its maximum (0 = strongest). 0.2 is AMD's default.
pub const SHARPNESS_STOPS: f32 = 0.2;

const EASU_MAIN: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
cbuffer F : register(b1) { float4 easu_in; float4 easu_out; float4 rcas_p; float4 rcas_q; };
AF4 FsrEasuRF(AF2 p) { return tex.GatherRed(smp, p, int2(0, 0)); }
AF4 FsrEasuGF(AF2 p) { return tex.GatherGreen(smp, p, int2(0, 0)); }
AF4 FsrEasuBF(AF2 p) { return tex.GatherBlue(smp, p, int2(0, 0)); }
"#;

const EASU_ENTRY: &str = r#"
float4 easu(float4 pos : SV_Position) : SV_Target {
    AU4 c0, c1, c2, c3;
    FsrEasuConOffset(c0, c1, c2, c3, easu_in.x, easu_in.y, easu_in.z, easu_in.w, easu_out.x, easu_out.y, easu_out.z, easu_out.w);
    AF3 c;
    FsrEasuF(c, AU2(pos.xy), c0, c1, c2, c3);
    return float4(c, 1);
}
"#;

const RCAS_MAIN: &str = r#"
Texture2D mid : register(t0);
cbuffer C : register(b0) { float4 uv; float4 bbox; float4 rects[8]; float4 ops[2]; float4 params; };
cbuffer F : register(b1) { float4 easu_in; float4 easu_out; float4 rcas_p; float4 rcas_q; };
AF4 FsrRcasLoadF(ASU2 p) { return mid.Load(int3(p, 0)); }
void FsrRcasInputF(inout AF1 r, inout AF1 g, inout AF1 b) {}
"#;

const RCAS_ENTRY: &str = r#"
float op(uint i) { return ops[i / 4][i % 4]; }
float4 rcas(float4 pos : SV_Position) : SV_Target {
    float2 ip = floor(pos.xy - rcas_p.yz);
    if (params.w > 0.5) {
        float2 f = bbox.xy + (ip + 0.5) / rcas_q.xy * bbox.zw;
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
    AU4 con;
    FsrRcasCon(con, rcas_p.x);
    AF1 r, g, b;
    FsrRcasF(r, g, b, AU2(ip), con);
    return float4(float3(r, g, b) * params.x, params.y);
}
"#;

#[repr(C)]
pub struct FsrConstants {
    /// Source region size (w, h) and the texture's full size (w, h), in pixels.
    pub easu_in: [f32; 4],
    /// Output size (w, h) and the region's offset in the texture (x, y), in pixels.
    pub easu_out: [f32; 4],
    /// Sharpness in stops, then the canvas position of the output (x, y).
    pub rcas_p: [f32; 4],
    /// Output size (w, h).
    pub rcas_q: [f32; 4],
}

pub struct Mid {
    pub tex: ID3D11Texture2D,
    pub rtv: ID3D11RenderTargetView,
    pub srv: ID3D11ShaderResourceView,
}

pub struct Fsr {
    pub easu: ID3D11PixelShader,
    pub rcas: ID3D11PixelShader,
    pub cbuf: ID3D11Buffer,
    mids: RefCell<HashMap<(u32, u32), Mid>>,
}

fn compile(src: &str, entry: PCSTR) -> Result<ID3DBlob> {
    unsafe {
        let mut code = None;
        let mut err = None;
        let r = D3DCompile(src.as_ptr() as *const _, src.len(), s!("fsr1.hlsl"), None, None, entry, s!("ps_5_0"), D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut code, Some(&mut err));
        if let Err(e) = r {
            if let Some(err) = err {
                let m = std::slice::from_raw_parts(err.GetBufferPointer() as *const u8, err.GetBufferSize());
                crate::log!("FSR shader: {}", String::from_utf8_lossy(m).lines().take(6).collect::<Vec<_>>().join(" | "));
            }
            return Err(e);
        }
        Ok(code.unwrap())
    }
}

/// The full shader source for one pass: AMD's headers with Kiritori's glue around them.
pub fn source(easu: bool) -> String {
    let define = if easu { "#define FSR_EASU_F 1\n" } else { "#define FSR_RCAS_F 1\n" };
    let (main, entry) = if easu { (EASU_MAIN, EASU_ENTRY) } else { (RCAS_MAIN, RCAS_ENTRY) };
    format!("#define A_GPU 1\n#define A_HLSL 1\n{define}{FFX_A}\n{main}\n{FFX_FSR1}\n{entry}")
}

impl Fsr {
    /// Compile both passes. Returns None (and Kiritori falls back to Smooth) if they fail.
    pub fn new(device: &ID3D11Device) -> Option<Self> {
        let started = std::time::Instant::now();
        let build = || -> Result<Fsr> {
            let e = compile(&source(true), s!("easu"))?;
            let r = compile(&source(false), s!("rcas"))?;
            let bytes = |b: &ID3DBlob| unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) };
            unsafe {
                let mut easu = None;
                device.CreatePixelShader(bytes(&e), None, Some(&mut easu))?;
                let mut rcas = None;
                device.CreatePixelShader(bytes(&r), None, Some(&mut rcas))?;
                let bd = D3D11_BUFFER_DESC {
                    ByteWidth: size_of::<FsrConstants>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                };
                let mut cbuf = None;
                device.CreateBuffer(&bd, None, Some(&mut cbuf))?;
                Ok(Fsr { easu: easu.unwrap(), rcas: rcas.unwrap(), cbuf: cbuf.unwrap(), mids: RefCell::new(HashMap::new()) })
            }
        };
        match build() {
            Ok(f) => {
                crate::log!("Enhanced scaling (FSR 1) ready in {} ms", started.elapsed().as_millis());
                Some(f)
            }
            Err(e) => {
                crate::log!("Enhanced scaling unavailable, using Smooth: {e}");
                None
            }
        }
    }

    /// A temporary image of this size for the upscaled cutout, kept for reuse.
    pub fn mid(&self, device: &ID3D11Device, size: (u32, u32)) -> Result<std::cell::Ref<'_, Mid>> {
        if !self.mids.borrow().contains_key(&size) {
            // Keep only a handful; cutout sizes change while resizing.
            if self.mids.borrow().len() > 8 {
                self.mids.borrow_mut().clear();
            }
            let d = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                ..Default::default()
            };
            unsafe {
                let mut t = None;
                device.CreateTexture2D(&d, None, Some(&mut t))?;
                let tex = t.unwrap();
                let mut rtv = None;
                device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
                let mut srv = None;
                device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
                self.mids.borrow_mut().insert(size, Mid { tex, rtv: rtv.unwrap(), srv: srv.unwrap() });
            }
        }
        Ok(std::cell::Ref::map(self.mids.borrow(), |m| &m[&size]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::*;

    /// A device on Windows' software renderer, so the test needs no graphics card.
    fn warp() -> (ID3D11Device, ID3D11DeviceContext) {
        let mut device = None;
        let mut ctx = None;
        unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx)).unwrap();
        }
        (device.unwrap(), ctx.unwrap())
    }

    #[test]
    fn both_passes_compile() {
        let (device, _) = warp();
        assert!(Fsr::new(&device).is_some());
    }

    /// Upscale a hard black/white edge 2x with EASU and read the result back.
    #[test]
    fn easu_keeps_an_edge_sharp() {
        let (device, ctx) = warp();
        let fsr = Fsr::new(&device).unwrap();
        let (sw, sh) = (32u32, 8u32);
        // Left half black, right half white.
        let px: Vec<u8> = (0..sw * sh).flat_map(|i| if i % sw < sw / 2 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
        unsafe {
            let d = D3D11_TEXTURE2D_DESC {
                Width: sw,
                Height: sh,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: sw * 4, SysMemSlicePitch: 0 };
            let mut src = None;
            device.CreateTexture2D(&d, Some(&init), Some(&mut src)).unwrap();
            let mut srv = None;
            device.CreateShaderResourceView(&src.unwrap(), None, Some(&mut srv)).unwrap();

            let vs_src = "struct V { float4 pos : SV_Position; }; V vs(uint id : SV_VertexID) { float2 q = float2(id & 1, id >> 1); V o; o.pos = float4(q.x * 2 - 1, 1 - q.y * 2, 0, 1); return o; }";
            let mut code = None;
            D3DCompile(vs_src.as_ptr() as *const _, vs_src.len(), None, None, None, s!("vs"), s!("vs_5_0"), 0, 0, &mut code, None).unwrap();
            let code = code.unwrap();
            let mut vs = None;
            device.CreateVertexShader(std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()), None, Some(&mut vs)).unwrap();
            let sd = D3D11_SAMPLER_DESC { Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR, AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, AddressV: D3D11_TEXTURE_ADDRESS_CLAMP, AddressW: D3D11_TEXTURE_ADDRESS_CLAMP, MaxLOD: f32::MAX, ..Default::default() };
            let mut smp = None;
            device.CreateSamplerState(&sd, Some(&mut smp)).unwrap();

            let (ow, oh) = (sw * 2, sh * 2);
            let mid = fsr.mid(&device, (ow, oh)).unwrap();
            let fc = FsrConstants { easu_in: [sw as f32, sh as f32, sw as f32, sh as f32], easu_out: [ow as f32, oh as f32, 0.0, 0.0], rcas_p: [SHARPNESS_STOPS, 0.0, 0.0, 0.0], rcas_q: [ow as f32, oh as f32, 0.0, 0.0] };
            ctx.UpdateSubresource(&fsr.cbuf, 0, None, &fc as *const _ as *const _, 0, 0);
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.VSSetShader(&vs.unwrap(), None);
            ctx.PSSetShader(&fsr.easu, None);
            ctx.PSSetConstantBuffers(1, Some(&[Some(fsr.cbuf.clone())]));
            ctx.PSSetShaderResources(0, Some(&[srv]));
            ctx.PSSetSamplers(0, Some(&[smp]));
            ctx.OMSetRenderTargets(Some(&[Some(mid.rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: ow as f32, Height: oh as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
            ctx.Draw(4, 0);

            // Read one row back.
            let rd = D3D11_TEXTURE2D_DESC { Width: ow, Height: oh, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..Default::default() };
            let mut staging = None;
            device.CreateTexture2D(&rd, None, Some(&mut staging)).unwrap();
            let staging = staging.unwrap();
            ctx.CopyResource(&staging, &mid.tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
            let row = std::slice::from_raw_parts((m.pData as *const u8).add(m.RowPitch as usize * (oh as usize / 2)), ow as usize * 4);
            let v: Vec<u8> = row.chunks(4).map(|p| p[1]).collect();
            ctx.Unmap(&staging, 0);

            assert!(v[..ow as usize / 2 - 3].iter().all(|x| *x < 10), "left stays black: {v:?}");
            assert!(v[ow as usize / 2 + 3..].iter().all(|x| *x > 245), "right stays white: {v:?}");
            // Plain bilinear at 2x spreads the edge over about 2 pixels; EASU must not be softer.
            let soft = v.iter().filter(|x| **x > 25 && **x < 230).count();
            assert!(soft <= 2, "edge is {soft} px wide: {v:?}");
        }
    }
}
