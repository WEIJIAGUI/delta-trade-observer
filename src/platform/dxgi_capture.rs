//! 桌面采集（毛玻璃背景源）：基于 DXGI Desktop Duplication API（Win8+ → Win10/Win11 通用）。
//!
//! 为什么不用 `DwmEnableBlurBehindWindow`：
//! 该 API 面向 GDI 分层窗口（`WS_EX_LAYERED` + 32bit DIB），而本程序的激活态窗口是
//! winit/egui 的 swapchain 透明窗口——DWM 无法从该窗口的交换链采样到有效背景，只能合成为纯黑，
//! 这正是「点击开始观察后先弹面板再黑屏」的根因。
//!
//! 本模块改用自采集方案：后台线程通过 DXGI 桌面复制把激活态窗口背后的桌面（游戏）抓取出来，
//! CPU 降采样 + 盒式模糊后产出一张 `egui::ColorImage`，由状态面板 / 导游面板按各自屏幕坐标
//! 裁切为该面板毛玻璃底图绘制。整个过程不经过任何 GPU 合成管线的模糊特性，故不会黑屏，
//! 且在 Win10 / Win11 上行为一致。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use egui::ColorImage;
use windows::core::ComInterface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_OUTPUT_DESC, DXGI_OUTDUPL_DESC, IDXGIFactory1, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// 降采样后的毛玻璃纹理目标宽度（像素）；高度按桌面宽高比推导。越大越清晰但越费 CPU。
const BLUR_TEX_W: u32 = 480;
/// 盒式模糊半径（像素，作用于降采样后的小图）。
const BLUR_RADIUS: usize = 2;

/// DXGI 桌面采集器：持有 D3D 设备 / 上下文 / 输出复制对象 / 暂存纹理。
struct DesktopDup {
    #[allow(dead_code)]
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    blur_w: usize,
    blur_h: usize,
    last: Option<ColorImage>,
}

impl DesktopDup {
    /// 为 `hwnd` 所在监视器创建桌面采集器。失败返回 windows 错误。
    fn new(hwnd: HWND) -> windows::core::Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(0)?;

            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_FLAG(0),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.unwrap();
            let context = context.unwrap();

            // 选择 hwnd 所在监视器对应的输出；找不到（或枚举完）则回退到第一个输出。
            let target_mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut chosen: Option<IDXGIOutput1> = None;
            for i in 0u32.. {
                let output = match adapter.EnumOutputs(i) {
                    Ok(o) => o,
                    Err(_) => break,
                };
                let mut desc: DXGI_OUTPUT_DESC = std::mem::zeroed();
                if output.GetDesc(&mut desc).is_ok() {
                    if desc.Monitor.0 != 0 && desc.Monitor.0 == target_mon.0 {
                        if let Ok(o1) = output.cast::<IDXGIOutput1>() {
                            chosen = Some(o1);
                            break;
                        }
                    }
                }
            }
            let output1 = match chosen {
                Some(o) => o,
                None => adapter.EnumOutputs(0)?.cast::<IDXGIOutput1>()?,
            };

            let duplication = output1.DuplicateOutput(&device)?;
            let mut dup_desc: DXGI_OUTDUPL_DESC = std::mem::zeroed();
            duplication.GetDesc(&mut dup_desc);
            let width = dup_desc.ModeDesc.Width;
            let height = dup_desc.ModeDesc.Height;

            // 与桌面同尺寸的 CPU 可读暂存纹理，用于把桌面帧拷出并映射读取。
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0u32,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            let staging = staging.unwrap();

            let aspect = height as f32 / width as f32;
            let blur_w = BLUR_TEX_W as usize;
            let blur_h = ((BLUR_TEX_W as f32 * aspect).round() as usize).max(1);

            Ok(Self {
                device,
                context,
                duplication,
                staging,
                width,
                height,
                blur_w,
                blur_h,
                // 首帧前的占位（深色调），避免纹理为空；真正像素在首帧 capture 后替换。
                last: Some(ColorImage::new(
                    [blur_w, blur_h],
                    egui::Color32::from_rgba_unmultiplied(20, 24, 34, 255),
                )),
            })
        }
    }

    /// 尝试抓取一帧并产出模糊后的桌面图像；若无新帧（DXGI 仍在绘制）则复用上一帧，维持不闪烁。
    fn capture(&mut self) -> windows::core::Result<&ColorImage> {
        unsafe {
            let mut frame_info: DXGI_OUTDUPL_FRAME_INFO = std::mem::zeroed();
            let mut resource: Option<IDXGIResource> = None;
            match self
                .duplication
                .AcquireNextFrame(0, &mut frame_info, &mut resource)
            {
                Ok(()) => {
                    if let Some(res) = resource {
                        if let Ok(tex) = res.cast::<ID3D11Texture2D>() {
                            self.context.CopyResource(&self.staging, &tex);
                            let mut mapped: D3D11_MAPPED_SUBRESOURCE = std::mem::zeroed();
                            if self
                                .context
                                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                                .is_ok()
                            {
                                let row_pitch = mapped.RowPitch as usize;
                                let src = std::slice::from_raw_parts(
                                    mapped.pData as *const u8,
                                    self.height as usize * row_pitch,
                                );
                                let img = Self::build_blur(
                                    src,
                                    row_pitch,
                                    self.width as usize,
                                    self.height as usize,
                                    self.blur_w,
                                    self.blur_h,
                                );
                                self.context.Unmap(&self.staging, 0);
                                self.last = Some(img);
                            }
                        }
                    }
                    let _ = self.duplication.ReleaseFrame();
                }
                Err(_) => {
                    // 暂无新帧（DXGI_ERROR_WAS_STILL_DRAWING 等）：复用上一帧。
                }
            }
            Ok(self.last.as_ref().unwrap())
        }
    }

    /// 把桌面 BGRA 整帧降采样到 `bw×bh`，并做一遍盒式模糊（进一步柔化）。
    fn build_blur(
        src: &[u8],
        row_pitch: usize,
        w: usize,
        h: usize,
        bw: usize,
        bh: usize,
    ) -> ColorImage {
        let sx = w as f32 / bw as f32;
        let sy = h as f32 / bh as f32;
        let mut small: Vec<u8> = vec![0u8; bw * bh * 4];
        for y in 0..bh {
            let y0 = (y as f32 * sy) as usize;
            let y1 = (((y + 1) as f32 * sy) as usize).min(h).max(y0 + 1);
            for x in 0..bw {
                let x0 = (x as f32 * sx) as usize;
                let x1 = (((x + 1) as f32 * sx) as usize).min(w).max(x0 + 1);
                let mut r = 0u32;
                let mut g = 0u32;
                let mut b = 0u32;
                let mut a = 0u32;
                let mut n = 0u32;
                for yy in y0..y1 {
                    let base = yy * row_pitch;
                    for xx in x0..x1 {
                        let o = base + xx * 4;
                        b += src[o] as u32;
                        g += src[o + 1] as u32;
                        r += src[o + 2] as u32;
                        a += src[o + 3] as u32;
                        n += 1;
                    }
                }
                if n > 0 {
                    let o = (y * bw + x) * 4;
                    small[o] = (b / n) as u8;
                    small[o + 1] = (g / n) as u8;
                    small[o + 2] = (r / n) as u8;
                    small[o + 3] = (a / n) as u8;
                }
            }
        }

        // 一遍盒式模糊（半径 BLUR_RADIUS），柔化降采样后的块状边缘。
        let radius = BLUR_RADIUS.min(bw / 2).min(bh / 2).max(1);
        let mut blurred: Vec<u8> = vec![0u8; bw * bh * 4];
        for y in 0..bh {
            for x in 0..bw {
                let x0 = x.saturating_sub(radius);
                let x1 = (x + radius).min(bw - 1);
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius).min(bh - 1);
                let mut r = 0u32;
                let mut g = 0u32;
                let mut b = 0u32;
                let mut n = 0u32;
                for yy in y0..=y1 {
                    let base = yy * bw;
                    for xx in x0..=x1 {
                        let o = (base + xx) * 4;
                        b += small[o] as u32;
                        g += small[o + 1] as u32;
                        r += small[o + 2] as u32;
                        n += 1;
                    }
                }
                let o = (y * bw + x) * 4;
                blurred[o] = (b / n) as u8;
                blurred[o + 1] = (g / n) as u8;
                blurred[o + 2] = (r / n) as u8;
                blurred[o + 3] = 255;
            }
        }

        ColorImage {
            size: [bw, bh],
            // small/blurred 均为 BGRA 顺序：r=p[2]、g=p[1]、b=p[0]。
            pixels: blurred
                .chunks_exact(4)
                .map(|p| egui::Color32::from_rgba_unmultiplied(p[2], p[1], p[0], 255))
                .collect(),
        }
    }
}

/// 桌面模糊采集控制器：在后台线程持续抓取并产出模糊桌面帧，主线程按需取用。
pub struct BlurCapture {
    slot: Arc<Mutex<Option<ColorImage>>>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BlurCapture {
    /// 启动后台采集线程，捕获 `hwnd` 所在监视器的桌面。
    pub fn start(hwnd: HWND) -> Self {
        let slot: Arc<Mutex<Option<ColorImage>>> = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let slot_t = slot.clone();
        let running_t = running.clone();
        let handle = thread::spawn(move || {
            // 后台线程初始化 COM 单元（桌面复制 API 在部分环境下需要）。
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut dup = match DesktopDup::new(hwnd) {
                Ok(d) => d,
                Err(e) => {
                    crate::log::write(&format!("[BLUR] 桌面采集初始化失败：{:?}", e));
                    return;
                }
            };
            while running_t.load(Ordering::Relaxed) {
                if let Ok(img) = dup.capture() {
                    *slot_t.lock().unwrap() = Some(img.clone());
                }
                thread::sleep(std::time::Duration::from_millis(60));
            }
        });
        Self {
            slot,
            running,
            handle: Some(handle),
        }
    }

    /// 取出最新一帧（克隆 CPU 端图像）。采集未就绪时返回 `None`。
    pub fn take(&self) -> Option<ColorImage> {
        self.slot.lock().unwrap().clone()
    }

    /// 停止后台线程并回收（join）。
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
