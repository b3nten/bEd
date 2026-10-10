//! Optional owned offscreen processing for native viewport surfaces.
use crate::wgpu;

/// Creates independent processing state for each native viewport. Called on the UI thread.
/// Implementations must produce an input view matching `input_format` with sample count one.
/// The backend retains surface ownership, texture reconciliation and presentation.
pub trait ViewportPostprocessorFactory: std::fmt::Debug {
    fn create(
        &self,
        device: &wgpu::Device,
        input_format: wgpu::TextureFormat,
        output_format: wgpu::TextureFormat,
        extent: [u32; 2],
    ) -> Result<Box<dyn ViewportPostprocessor>, String>;
}

/// Owned by a single live viewport allocation, released before its native surface.
/// No ImGui/context pointers or render-pass borrows escape these calls.
pub trait ViewportPostprocessor {
    /// Resize/reset as needed and return the input target for this frame.
    fn input_view(
        &mut self,
        device: &wgpu::Device,
        extent: [u32; 2],
    ) -> Result<wgpu::TextureView, String>;
    /// Encode processing after ImGui has rendered the input, before backend submission.
    fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) -> Result<(), String>;
}
