//! Quarantine a panel after an unwinding panic. Its document and saved view
//! state remain host-owned, but no further callbacks enter the damaged panel.
use bed_document_session::DocumentId;
use bed_workbench_api::{
    EditToken, ExternalFileDrag, ExternalFileDropResponse, HostContext, HostRequest, ModulePanel,
    ModuleServices, PanelAction, Revision,
    gpu::{GpuContext, RenderOutput, RenderTarget},
};
use dear_imgui_rs::{Ui, sys};
use serde_json::Value;
use std::{
    any::Any,
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
};

pub(super) struct PanelBoundary {
    inner: Box<dyn ModulePanel>,
    kind: String,
    failure: RefCell<Option<String>>,
    state: RefCell<Value>,
    document: Option<DocumentId>,
    view: Option<bed_document_session::ViewId>,
    persist: bool,
}

impl PanelBoundary {
    pub fn new(kind: &str, inner: Box<dyn ModulePanel>, state: &Value) -> Self {
        Self {
            document: inner.attached_document(),
            view: inner.view_id(),
            persist: inner.persist(),
            inner,
            kind: kind.to_owned(),
            failure: RefCell::new(None),
            state: RefCell::new(state.clone()),
        }
    }
}

fn call<T>(
    kind: &str,
    failure: &RefCell<Option<String>>,
    operation: &str,
    callback: impl FnOnce() -> T,
) -> Result<T, String> {
    if let Some(error) = failure.borrow().as_ref() {
        return Err(error.clone());
    }
    // The failed instance is quarantined instead of attempting to resume it.
    // Host requests from a panicking callback are discarded by its caller.
    catch_unwind(AssertUnwindSafe(callback)).map_err(|payload| {
        let detail = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-text panic payload");
        let error = format!("Extension panel {kind} panicked during {operation}: {detail}");
        *failure.borrow_mut() = Some(error.clone());
        error
    })
}

impl ModulePanel for PanelBoundary {
    fn is_input_empty(&self, host: &HostContext<'_>) -> bool {
        call(&self.kind, &self.failure, "empty input", || {
            self.inner.is_input_empty(host)
        })
        .unwrap_or(false)
    }
    fn attached_document(&self) -> Option<DocumentId> {
        self.document
    }
    fn view_id(&self) -> Option<bed_document_session::ViewId> {
        self.view
    }
    fn persist(&self) -> bool {
        self.persist
    }
    fn title(&self, host: &HostContext<'_>) -> String {
        call(&self.kind, &self.failure, "title", || {
            self.inner.title(host)
        })
        .unwrap_or_else(|_| self.kind.clone())
    }
    fn window_padding(&self) -> Option<[f32; 2]> {
        call(&self.kind, &self.failure, "window padding", || {
            self.inner.window_padding()
        })
        .unwrap_or(None)
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _ = self.draw_panel(ui, requests, |inner, requests| {
            inner.draw(ui, host, requests);
            Ok(())
        });
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.draw_panel(ui, requests, |inner, requests| {
            inner.draw_with_services(ui, host, services, requests)
        })
    }
    fn render_output(&self) -> Option<RenderOutput> {
        call(&self.kind, &self.failure, "render output", || {
            self.inner.render_output()
        })
        .unwrap_or(None)
    }
    fn render(&mut self, gpu: &mut GpuContext<'_>, target: &RenderTarget) -> Result<(), String> {
        call(&self.kind, &self.failure, "render", || {
            self.inner.render(gpu, target)
        })?
    }
    fn action(
        &mut self,
        action: PanelAction,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        if self.failure.borrow().is_some() {
            return Ok(false);
        }
        self.requests("action", requests, |inner, requests| {
            inner.action(action, host, requests)
        })?
    }
    fn action_with_services(
        &mut self,
        action: PanelAction,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        if self.failure.borrow().is_some() {
            return Ok(false);
        }
        self.requests("action", requests, |inner, requests| {
            inner.action_with_services(action, host, services, requests)
        })?
    }
    fn focus_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<()> {
        // A failed panel stays focusable so its error can be read and it can be closed.
        if self.failure.borrow().is_some() {
            return Ok(());
        }
        call(&self.kind, &self.failure, "focus", || {
            self.inner.focus_with_services(services)
        })
        .map_err(io::Error::other)?
    }
    fn edit_result(&mut self, token: EditToken, result: Result<Revision, String>) {
        let _ = call(&self.kind, &self.failure, "edit result", || {
            self.inner.edit_result(token, result)
        });
    }
    fn document_removed_with_services(
        &mut self,
        document: DocumentId,
        path: &str,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if self.failure.borrow().is_some() {
            return Ok(());
        }
        self.requests("document removed", requests, |inner, requests| {
            inner.document_removed_with_services(document, path, services, requests)
        })
        .map_err(io::Error::other)?
    }
    fn external_files_with_services(
        &mut self,
        event: &ExternalFileDrag,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<ExternalFileDropResponse> {
        if self.failure.borrow().is_some() {
            return Ok(ExternalFileDropResponse::Ignored);
        }
        self.requests("file drop", requests, |inner, requests| {
            inner.external_files_with_services(event, host, services, requests)
        })
        .map_err(io::Error::other)?
    }
    fn save_state(&self) -> Value {
        if let Ok(state) = call(&self.kind, &self.failure, "save state", || {
            self.inner.save_state()
        }) {
            *self.state.borrow_mut() = state;
        }
        self.state.borrow().clone()
    }
    fn save_state_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<Value> {
        if self.failure.borrow().is_some() {
            return Ok(self.state.borrow().clone());
        }
        match call(&self.kind, &self.failure, "save state", || {
            self.inner.save_state_with_services(services)
        }) {
            Ok(result) => {
                let state = result?;
                *self.state.borrow_mut() = state.clone();
                Ok(state)
            }
            Err(_) => Ok(self.state.borrow().clone()),
        }
    }
    fn close(&mut self, requests: &mut Vec<HostRequest>) {
        let _ = self.requests("close", requests, |inner, requests| inner.close(requests));
    }
    fn close_with_services(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if self.failure.borrow().is_some() {
            return Ok(());
        }
        // A panic during close must not trap a failed tab in the workspace.
        match self.requests("close", requests, |inner, requests| {
            inner.close_with_services(services, requests)
        }) {
            Ok(result) => result,
            Err(message) => {
                requests.push(HostRequest::Notify { message });
                Ok(())
            }
        }
    }
    fn as_any(&self) -> &dyn Any {
        if self.failure.borrow().is_some() {
            self
        } else {
            self.inner.as_any()
        }
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        if self.failure.borrow().is_some() {
            self
        } else {
            self.inner.as_any_mut()
        }
    }
}

impl PanelBoundary {
    fn requests<T>(
        &mut self,
        operation: &str,
        requests: &mut Vec<HostRequest>,
        callback: impl FnOnce(&mut dyn ModulePanel, &mut Vec<HostRequest>) -> T,
    ) -> Result<T, String> {
        let mut pending = Vec::new();
        let result = call(&self.kind, &self.failure, operation, || {
            callback(self.inner.as_mut(), &mut pending)
        });
        if result.is_ok() {
            requests.extend(pending);
        }
        result
    }

    fn draw_panel(
        &mut self,
        ui: &Ui,
        requests: &mut Vec<HostRequest>,
        callback: impl FnOnce(&mut dyn ModulePanel, &mut Vec<HostRequest>) -> io::Result<()>,
    ) -> io::Result<()> {
        if let Some(error) = self.failure.borrow().as_ref() {
            ui.text_wrapped(error);
            ui.text_wrapped("Close and reopen this panel to try again.");
            return Ok(());
        }
        ui.with_bound_context(|| {
            let mut stacks = sys::ImGuiErrorRecoveryState::default();
            unsafe { sys::igErrorRecoveryStoreState(&mut stacks) };
            match self.requests("draw", requests, callback) {
                Ok(result) => result,
                Err(error) => {
                    // Rust RAII tokens unwind first. Recover raw ImGui scopes
                    // that the extension may have left open before panicking.
                    unsafe {
                        let io = sys::igGetIO_Nil();
                        let assert = (*io).ConfigErrorRecoveryEnableAssert;
                        (*io).ConfigErrorRecoveryEnableAssert = false;
                        sys::igErrorRecoveryTryToRecoverState(&stacks);
                        (*io).ConfigErrorRecoveryEnableAssert = assert;
                    }
                    ui.text_wrapped(error);
                    ui.text_wrapped("Close and reopen this panel to try again.");
                    Ok(())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench_api::{TextureHandle, gpu::wgpu};
    use std::{
        cell::Cell,
        future::Future,
        rc::Rc,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    struct Canvas {
        panic: bool,
        renders: Rc<Cell<usize>>,
    }
    impl ModulePanel for Canvas {
        fn title(&self, _: &HostContext<'_>) -> String {
            "Canvas".into()
        }
        fn draw(&mut self, _: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {}
        fn render_output(&self) -> Option<RenderOutput> {
            Some(RenderOutput {
                handle: TextureHandle(1),
                size: [16; 2],
                depth: false,
                revision: 1,
            })
        }
        fn render(
            &mut self,
            gpu: &mut GpuContext<'_>,
            target: &RenderTarget,
        ) -> Result<(), String> {
            self.renders.set(self.renders.get() + 1);
            let _pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            assert!(!self.panic, "canvas render panic");
            Ok(())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn native_panel_render_panic_keeps_shared_device_usable() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = block_on(adapter.request_device(&Default::default())).unwrap();
        let renders = Rc::new(Cell::new(0));
        let mut panel = PanelBoundary::new(
            "test.canvas",
            Box::new(Canvas {
                panic: true,
                renders: Rc::clone(&renders),
            }),
            &Value::Null,
        );
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let target = RenderTarget::new(&device, panel.render_output().unwrap()).unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut gpu = GpuContext {
                instance: &instance,
                adapter: &adapter,
                device: &device,
                queue: &queue,
                encoder: &mut encoder,
                generation: 1,
                error_handlers: None,
            };
            let error = panel.render(&mut gpu, &target).unwrap_err();
            assert!(error.contains("canvas render panic"));
            assert!(panel.render_output().is_none());
            assert!(panel.render(&mut gpu, &target).is_err());
            assert_eq!(renders.get(), 1);
        }
        drop(encoder);

        let mut healthy = PanelBoundary::new(
            "test.healthy",
            Box::new(Canvas {
                panic: false,
                renders: Rc::clone(&renders),
            }),
            &Value::Null,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        healthy
            .render(
                &mut GpuContext {
                    instance: &instance,
                    adapter: &adapter,
                    device: &device,
                    queue: &queue,
                    encoder: &mut encoder,
                    generation: 1,
                    error_handlers: None,
                },
                &target,
            )
            .unwrap();
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        assert_eq!(renders.get(), 2);
        assert!(block_on(validation.pop()).is_none());
    }
}
