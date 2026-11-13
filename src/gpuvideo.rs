// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Вывод кадров своего проигрывателя HEVC через Direct3D 11: плоскости
//! Y, U, V уходят в видеокарту как есть, а перевод цветов (BT.709),
//! поворот и плавное масштабирование делает шейдер. Процессор свободен
//! для расшифровки. Если видеокарты/драйвера нет — показ через GDI.

use std::ffi::c_void;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const HLSL: &str = r#"
cbuffer C : register(b0) { float4 c0; float4 c1; float4 c2; float4 c3; float4 k; };
struct O { float4 p : SV_Position; float2 t : TEXCOORD0; };
O vs(uint id : SV_VertexID) {
    O o;
    float x = (float)(id & 1u);
    float y = (float)(id >> 1u);
    o.p = float4(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    // углы без индекса по массиву (так собирают и старые компиляторы)
    o.t = lerp(lerp(c0.xy, c1.xy, x), lerp(c2.xy, c3.xy, x), y);
    return o;
}
Texture2D Y : register(t0);
Texture2D U : register(t1);
Texture2D V : register(t2);
SamplerState S : register(s0);
float4 ps(O i) : SV_Target {
    float y = Y.Sample(S, i.t).r * k.x;
    float u = U.Sample(S, i.t).r * k.y;
    float v = V.Sample(S, i.t).r * k.y;
    u = lerp(u, 128.0 / 255.0, k.z);
    v = lerp(v, 128.0 / 255.0, k.z);
    y = (y - 16.0 / 255.0) * (255.0 / 219.0);
    u = (u - 128.0 / 255.0) * (255.0 / 224.0);
    v = (v - 128.0 / 255.0) * (255.0 / 224.0);
    float3 c = float3(y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u);
    return float4(saturate(c), 1.0);
}
"#;

type D3DCompileFn = unsafe extern "system" fn(
    *const c_void,
    usize,
    PCSTR,
    *const c_void,
    *const c_void,
    PCSTR,
    PCSTR,
    u32,
    u32,
    *mut *mut c_void,
    *mut *mut c_void,
) -> HRESULT;

/// Шейдер из текста — d3dcompiler_47.dll (есть в Windows 8.1/10/11);
/// грузим сами, чтобы без неё программа всё равно запускалась.
fn compile(entry: &str, target: &str) -> Option<Vec<u8>> {
    compile_src(HLSL, entry, target)
}

fn compile_src(src: &str, entry: &str, target: &str) -> Option<Vec<u8>> {
    unsafe {
        let lib = LoadLibraryW(w!("d3dcompiler_47.dll")).ok()?;
        let f = GetProcAddress(lib, s!("D3DCompile"))?;
        let f: D3DCompileFn = std::mem::transmute(f);
        let e = format!("{entry}\0");
        let t = format!("{target}\0");
        let mut code: *mut c_void = std::ptr::null_mut();
        let mut err: *mut c_void = std::ptr::null_mut();
        let hr = f(src.as_ptr() as _, src.len(), s!("spv"), std::ptr::null(), std::ptr::null(), PCSTR(e.as_ptr()), PCSTR(t.as_ptr()), 1 << 15, 0, &mut code, &mut err);
        if !err.is_null() {
            drop(ID3DBlob::from_raw(err));
        }
        if hr.is_err() || code.is_null() {
            return None;
        }
        let b = ID3DBlob::from_raw(code);
        Some(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec())
    }
}

struct Plane {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    w: usize,
    h: usize,
}

pub struct Gpu {
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    sc: IDXGISwapChain,
    rtv: Option<ID3D11RenderTargetView>,
    size: (u32, u32),
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    smp: ID3D11SamplerState,
    cb: ID3D11Buffer,
    planes: [Option<Plane>; 3],
}

impl Gpu {
    pub fn new(hwnd: HWND, w: i32, h: i32) -> Option<Gpu> {
        if std::env::var("SPV_NO_GPU").is_ok() {
            return None;
        }
        let vsb = compile("vs", "vs_4_0")?;
        let psb = compile("ps", "ps_4_0")?;
        unsafe {
            let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
            let mut made = None;
            // сначала современный «flip» (Windows 10+), потом классический
            for (fx, n) in [(DXGI_SWAP_EFFECT_FLIP_DISCARD, 2), (DXGI_SWAP_EFFECT_DISCARD, 1)] {
                let desc = DXGI_SWAP_CHAIN_DESC {
                    BufferDesc: DXGI_MODE_DESC { Width: w.max(1) as u32, Height: h.max(1) as u32, Format: DXGI_FORMAT_B8G8R8A8_UNORM, ..Default::default() },
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: n,
                    OutputWindow: hwnd,
                    Windowed: TRUE,
                    SwapEffect: fx,
                    Flags: 0,
                };
                let (mut sc, mut dev, mut ctx) = (None, None, None);
                if D3D11CreateDeviceAndSwapChain(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&levels),
                    D3D11_SDK_VERSION,
                    Some(&desc),
                    Some(&mut sc),
                    Some(&mut dev),
                    None,
                    Some(&mut ctx),
                )
                .is_ok()
                {
                    if let (Some(sc), Some(dev), Some(ctx)) = (sc, dev, ctx) {
                        made = Some((sc, dev, ctx));
                        break;
                    }
                }
            }
            let (sc, dev, ctx) = made?;
            // Alt+Enter DXGI не должен перехватывать
            if let Ok(f) = sc.GetParent::<IDXGIFactory>() {
                let _ = f.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES);
            }
            let mut vs = None;
            dev.CreateVertexShader(&vsb, None, Some(&mut vs)).ok()?;
            let mut ps = None;
            dev.CreatePixelShader(&psb, None, Some(&mut ps)).ok()?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut smp = None;
            dev.CreateSamplerState(&sd, Some(&mut smp)).ok()?;
            let bd = D3D11_BUFFER_DESC { ByteWidth: 80, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
            let mut cb = None;
            dev.CreateBuffer(&bd, None, Some(&mut cb)).ok()?;
            let mut g = Gpu { dev, ctx, sc, rtv: None, size: (w.max(1) as u32, h.max(1) as u32), vs: vs?, ps: ps?, smp: smp?, cb: cb?, planes: [None, None, None] };
            g.make_rtv().then_some(g)
        }
    }

    fn make_rtv(&mut self) -> bool {
        unsafe {
            let Ok(buf) = self.sc.GetBuffer::<ID3D11Texture2D>(0) else { return false };
            let mut rtv = None;
            if self.dev.CreateRenderTargetView(&buf, None, Some(&mut rtv)).is_err() {
                return false;
            }
            self.rtv = rtv;
            self.rtv.is_some()
        }
    }

    fn resize(&mut self, w: u32, h: u32) -> bool {
        if (w, h) == self.size {
            return true;
        }
        unsafe {
            self.ctx.OMSetRenderTargets(None, None);
            self.rtv = None;
            if self.sc.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)).is_err() {
                return false;
            }
        }
        self.size = (w, h);
        self.make_rtv()
    }

    fn upload(&mut self, i: usize, p: &rusty_h265::Plane) -> bool {
        unsafe {
            let fits = self.planes[i].as_ref().map(|q| q.w == p.width && q.h == p.height).unwrap_or(false);
            if !fits {
                self.planes[i] = None;
                let td = D3D11_TEXTURE2D_DESC {
                    Width: p.width as u32,
                    Height: p.height as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R16_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                };
                let mut tex = None;
                if self.dev.CreateTexture2D(&td, None, Some(&mut tex)).is_err() {
                    return false;
                }
                let Some(tex) = tex else { return false };
                let mut srv = None;
                if self.dev.CreateShaderResourceView(&tex, None, Some(&mut srv)).is_err() {
                    return false;
                }
                let Some(srv) = srv else { return false };
                self.planes[i] = Some(Plane { tex, srv, w: p.width, h: p.height });
            }
            let q = self.planes[i].as_ref().unwrap();
            if p.data.len() < p.stride * p.height {
                return false;
            }
            self.ctx.UpdateSubresource(&q.tex, 0, None, p.data.as_ptr() as *const c_void, (p.stride * 2) as u32, 0);
            true
        }
    }

    /// Загрузить кадр в видеокарту (только когда пришёл новый).
    pub fn load(&mut self, fr: &rusty_h265::Frame) -> bool {
        let p = &fr.picture;
        let n = if p.chroma_format_idc == 0 { 1 } else { 3 };
        (0..n).all(|i| self.upload(i, &p.planes[i]))
    }

    /// Нарисовать загруженный кадр, вписав в окно (поля — чёрные). false — сломалось, нужен GDI.
    pub fn draw(&mut self, fr: &rusty_h265::Frame, rot: u32, (cw, ch): (i32, i32)) -> bool {
        let (cw, ch) = (cw.max(1) as u32, ch.max(1) as u32);
        if !self.resize(cw, ch) {
            return false;
        }
        let p = &fr.picture;
        let Some(y) = self.planes[0].as_ref() else { return false };
        let (lw, lh) = (y.w as f32, y.h as f32);
        let (cx, cy, w, h) = p.crop;
        let (u0, v0, u1, v1) = (cx as f32 / lw, cy as f32 / lh, (cx + w) as f32 / lw, (cy + h) as f32 / lh);
        let (tl, tr, bl, br) = ([u0, v0], [u1, v0], [u0, v1], [u1, v1]);
        // углы экрана: левый верх, правый верх, левый низ, правый низ
        let corners = match rot {
            90 => [bl, tl, br, tr],
            180 => [br, bl, tr, tl],
            270 => [tr, br, tl, bl],
            _ => [tl, tr, bl, br],
        };
        let (sw, sh) = if rot % 180 == 90 { (h, w) } else { (w, h) };
        let k = (cw as f64 / sw as f64).min(ch as f64 / sh as f64);
        let (vw, vh) = ((sw as f64 * k).round(), (sh as f64 * k).round());
        let (vx, vy) = (((cw as f64 - vw) / 2.0).floor(), ((ch as f64 - vh) / 2.0).floor());
        let shift = (p.bit_depth_luma as i32 - 8).max(0);
        let scale = 65535.0 / (255.0 * (1 << shift) as f32);
        let mut cbuf = [0f32; 20];
        for (i, c) in corners.iter().enumerate() {
            cbuf[i * 4] = c[0];
            cbuf[i * 4 + 1] = c[1];
        }
        cbuf[16] = scale;
        cbuf[17] = 65535.0 / (255.0 * (1 << (p.bit_depth_chroma as i32 - 8).max(0)) as f32);
        cbuf[18] = if p.chroma_format_idc == 0 { 1.0 } else { 0.0 };
        unsafe {
            let ctx = &self.ctx;
            ctx.UpdateSubresource(&self.cb, 0, None, cbuf.as_ptr() as *const c_void, 0, 0);
            ctx.OMSetRenderTargets(Some(&[self.rtv.clone()]), None);
            if let Some(rtv) = &self.rtv {
                ctx.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0]);
            }
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: vx as f32, TopLeftY: vy as f32, Width: vw as f32, Height: vh as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            let y = self.planes[0].as_ref().map(|q| q.srv.clone());
            let srvs = if p.chroma_format_idc == 0 { [y.clone(), y.clone(), y] } else { [y, self.planes[1].as_ref().map(|q| q.srv.clone()), self.planes[2].as_ref().map(|q| q.srv.clone())] };
            ctx.PSSetShaderResources(0, Some(&srvs));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.Draw(4, 0);
            let hr = self.sc.Present(1, DXGI_PRESENT(0));
            hr.is_ok()
        }
    }
}

#[cfg(test)]
mod tests {
    /// Шейдеры должны собираться системным компилятором.
    #[test]
    fn hlsl_probe() {
        if let Ok(p) = std::env::var("SPV_HLSL") {
            let src = std::fs::read_to_string(p).unwrap();
            let e = std::env::var("SPV_HLSL_E").unwrap();
            let t = std::env::var("SPV_HLSL_T").unwrap();
            assert!(super::compile_src(&src, &e, &t).is_some());
        }
    }

    #[test]
    fn shaders_compile() {
        assert!(super::compile("vs", "vs_4_0").is_some());
        assert!(super::compile("ps", "ps_4_0").is_some());
    }
}
