use crate::panels::PanelPolicy;
use bedtime_http_client::{HttpClientPanel, HttpClientPlugin};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, PanelFactory,
};
use bevy::prelude::*;

pub struct OpenHttpClient;
impl Action for OpenHttpClient {
    const KEY: &'static str = "app.open_http_client";
    const LABEL: &'static str = "HTTP Client";
}

impl PanelPolicy for HttpClientPanel {
    /// One HTTP client shows the workspace's requests.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

pub struct BedHttpClientPlugin;

impl Plugin for BedHttpClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HttpClientPlugin)
            .add_action_handler::<OpenHttpClient>(Handler::new(open_http_client));
    }
}

fn open_http_client(world: &mut World, _: ActionInvocation) -> ActionResult {
    crate::panels::open_panel::<HttpClientPanel>(world, (), PanelFactory::of::<HttpClientPanel>())
        .map(|_| ())
}
