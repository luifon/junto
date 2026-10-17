//! The picture of a share with Windows Graphics Capture (ADR 0706).
//!
//! The system gives each picture as a texture of the graphics card. The capture copies the
//! newest one into a texture of its own on the card, which costs no work of the processor,
//! and tells the delivery that a picture waits. The delivery reads that texture into memory
//! only for a picture that it sends, at the chosen frame rate.

use std::sync::{Arc, Mutex, PoisonError};

use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface};

use super::capture::Thread;
use crate::share::CaptureError;
use crate::share::delivery::pixels::{Format, Frame};
use crate::share::delivery::{Latest, Life};

/// What a share shows.
#[derive(Clone, Copy)]
pub(super) enum Target {
    Monitor(HMONITOR),
    Window(HWND),
}

// SAFETY: a handle of a screen or a window is a number that any thread may pass to the
// system.
unsafe impl Send for Target {}

fn failed(error: windows::core::Error) -> CaptureError {
    CaptureError::Failed(error.message())
}

struct Textures {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// The newest picture, copied out of the buffer of the capture, with its size.
    latest: Option<(ID3D11Texture2D, (u32, u32))>,
    /// The texture from which the processor reads a picture.
    staging: Option<(ID3D11Texture2D, (u32, u32))>,
    /// A screen has no transparent pixels; a window has some at its rounded corners.
    format: Format,
}

/// The textures of one share, used by the threads of the system that give the pictures and
/// by the delivery thread.
pub(super) struct Gpu(Mutex<Textures>);

// SAFETY: a Direct3D 11 device may be used from any thread. Its immediate context may not be
// used by two threads at once: every use of it is inside the lock of `Gpu`, and the context
// is also protected by the device's own lock (`SetMultithreadProtected`).
unsafe impl Send for Gpu {}
// SAFETY: as above; the textures are reached only through the lock.
unsafe impl Sync for Gpu {}

fn texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    staging: bool,
) -> Result<ID3D11Texture2D, CaptureError> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: 0,
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut texture = None;
    // SAFETY: the description is complete, and the call writes the texture into `texture`.
    unsafe { device.CreateTexture2D(&description, None, Some(&mut texture)) }.map_err(failed)?;
    texture.ok_or_else(|| CaptureError::Failed("no texture".to_owned()))
}

impl Gpu {
    /// Copies the visible part of a picture of the capture into the texture of the newest
    /// picture.
    fn keep(&self, source: &ID3D11Texture2D, size: (u32, u32)) -> Result<(), CaptureError> {
        let mut textures = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if textures.latest.as_ref().map(|latest| latest.1) != Some(size) {
            textures.latest = Some((texture(&textures.device, size, false)?, size));
        }
        let Some((latest, _)) = &textures.latest else {
            return Ok(());
        };
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: size.0,
            bottom: size.1,
            back: 1,
        };
        // SAFETY: both textures belong to the device of the context and have the same
        // format; the region lies inside both, because the size is at most the size of the
        // source and is the size of the destination.
        unsafe {
            textures
                .context
                .CopySubresourceRegion(latest, 0, 0, 0, 0, source, 0, Some(&region));
        }
        Ok(())
    }

    /// Reads the newest picture into memory.
    pub(super) fn read(&self) -> Result<Option<Frame>, CaptureError> {
        let mut textures = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((latest, size)) = textures.latest.clone() else {
            return Ok(None);
        };
        if textures.staging.as_ref().map(|staging| staging.1) != Some(size) {
            textures.staging = Some((texture(&textures.device, size, true)?, size));
        }
        let Some((staging, _)) = textures.staging.clone() else {
            return Ok(None);
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures belong to the device of the context and have one size and
        // one format. The mapped memory holds `RowPitch` bytes for each of the `size.1` rows
        // and is read before `Unmap`.
        unsafe {
            textures.context.CopyResource(&staging, &latest);
            textures
                .context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(failed)?;
            let pitch = mapped.RowPitch as usize;
            let length = pitch * (size.1 as usize - 1) + size.0 as usize * 4;
            let bytes = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), length);
            let frame = Frame::copy(
                bytes,
                size,
                i32::try_from(pitch).unwrap_or(i32::MAX),
                None,
                textures.format,
            );
            textures.context.Unmap(&staging, 0);
            frame.map(Some)
        }
    }
}

/// The values that the handler of the pictures holds. The handler runs on threads of the
/// system.
struct Arrival {
    device: IDirect3DDevice,
    size: SizeInt32,
    gpu: Arc<Gpu>,
    pictures: Arc<Latest<()>>,
    life: Arc<Life>,
}

// SAFETY: the Direct3D device of Windows Runtime wraps a device that may be used from any
// thread.
unsafe impl Send for Arrival {}

impl Arrival {
    /// Takes the newest picture of the pool. The pool was made with `CreateFreeThreaded`, so
    /// its methods may be called on the thread of the handler.
    fn take(&mut self, pool: &Direct3D11CaptureFramePool) -> Result<(), CaptureError> {
        let frame = pool.TryGetNextFrame().map_err(failed)?;
        let content = frame.ContentSize().map_err(failed)?;
        let surface = frame.Surface().map_err(failed)?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(failed)?;
        // SAFETY: the surface of a frame of the pool is a Direct3D 11 texture.
        let source: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(failed)?;
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the call writes the description of the texture.
        unsafe { source.GetDesc(&mut description) };
        // A window that became smaller has its picture in the top left part of a larger
        // buffer until the pool has buffers of its new size.
        let size = (
            (content.Width.max(0) as u32).min(description.Width),
            (content.Height.max(0) as u32).min(description.Height),
        );
        if size.0 > 0 && size.1 > 0 {
            self.gpu.keep(&source, size)?;
            self.pictures.put(());
        }
        let _ = frame.Close();
        if content.Width != self.size.Width || content.Height != self.size.Height {
            pool.Recreate(
                &self.device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                content,
            )
            .map_err(failed)?;
            self.size = content;
        }
        Ok(())
    }
}

/// The objects of a running capture.
pub(super) struct Session {
    pub gpu: Arc<Gpu>,
    /// The size of the source in pixels at the start.
    pub size: (u32, u32),
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    item: GraphicsCaptureItem,
    arrived: i64,
    closed: i64,
}

impl Session {
    /// Starts the capture of a screen or a window. The calling thread runs a message loop
    /// for as long as the capture runs: the system tells the end of a window through it.
    pub(super) fn start(
        target: Target,
        life: Arc<Life>,
        pictures: Arc<Latest<()>>,
        owner: Thread,
    ) -> Result<Self, CaptureError> {
        let mut device = None;
        let mut context = None;
        // SAFETY: the call writes the device and its immediate context into the values.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(failed)?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err(CaptureError::Failed("no Direct3D device".to_owned()));
        };
        let protected: ID3D11Multithread = context.cast().map_err(failed)?;
        // SAFETY: the call changes a setting of the device.
        let _ = unsafe { protected.SetMultithreadProtected(true) };
        let dxgi: IDXGIDevice = device.cast().map_err(failed)?;
        // SAFETY: the DXGI device belongs to a Direct3D 11 device.
        let inspectable: IInspectable =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.map_err(failed)?;
        let direct3d: IDirect3DDevice = inspectable.cast().map_err(failed)?;

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(failed)?;
        // SAFETY: the handle is a screen or a window that the lists of the system gave; a
        // handle that is not valid any more makes the call fail.
        let item: GraphicsCaptureItem = unsafe {
            match target {
                Target::Monitor(monitor) => interop.CreateForMonitor(monitor),
                Target::Window(window) => interop.CreateForWindow(window),
            }
        }
        .map_err(|_| CaptureError::SourceGone)?;
        let size = item.Size().map_err(failed)?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(CaptureError::SourceGone);
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct3d,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(failed)?;
        let session = pool.CreateCaptureSession(&item).map_err(failed)?;
        session.SetIsCursorCaptureEnabled(true).map_err(failed)?;

        let gpu = Arc::new(Gpu(Mutex::new(Textures {
            device,
            context,
            latest: None,
            staging: None,
            format: match target {
                Target::Monitor(_) => Format::Bgrx,
                Target::Window(_) => Format::Bgra,
            },
        })));
        let closed = item
            .Closed(&TypedEventHandler::new({
                let (life, pictures) = (life.clone(), pictures.clone());
                move |_, _| {
                    // The window was closed, or the screen was removed.
                    life.fail(CaptureError::SourceGone);
                    pictures.wake();
                    owner.wake();
                    Ok(())
                }
            }))
            .map_err(failed)?;
        // The system calls the handler on one thread at a time; the lock holds the size
        // of the pool between two calls.
        // The handler holds no reference to the pool: the pool holds the handler until it is
        // closed, and a reference back would keep both alive after the share.
        let arrival = Mutex::new(Arrival {
            device: direct3d,
            size,
            gpu: gpu.clone(),
            pictures,
            life,
        });
        let arrived = pool
            .FrameArrived(&TypedEventHandler::new(
                move |pool: windows::core::Ref<'_, Direct3D11CaptureFramePool>, _| {
                    let mut arrival = arrival.lock().unwrap_or_else(PoisonError::into_inner);
                    if arrival.life.stopped() {
                        return Ok(());
                    }
                    if let Err(failure) = pool
                        .ok()
                        .map_err(failed)
                        .and_then(|pool| arrival.take(pool))
                    {
                        arrival.life.fail(failure);
                        arrival.pictures.wake();
                        owner.wake();
                    }
                    Ok(())
                },
            ))
            .map_err(failed)?;
        let running = Self {
            gpu,
            size: (size.Width as u32, size.Height as u32),
            session,
            pool,
            item,
            arrived,
            closed,
        };
        running.session.StartCapture().map_err(failed)?;
        Ok(running)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.arrived);
        let _ = self.item.RemoveClosed(self.closed);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
