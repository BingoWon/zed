use gpui::{
    div, Context, Entity, IntoElement, ParentElement, Render, Window, Subscription,
};
use ui::{prelude::*, IconButton, IconName, IconSize, Tooltip};
use workspace::{item::ItemHandle, StatusItemView};

use crate::{ui::ToggleFocus, ZedVisionSync, ServiceEvent};

pub struct ZedVisionStatusButton {
    sync_service: Entity<ZedVisionSync>,
    is_service_running: bool,
    _subscription: Subscription,
}

impl ZedVisionStatusButton {
    pub fn new(sync_service: Entity<ZedVisionSync>, cx: &mut Context<Self>) -> Self {
        // 订阅服务状态变化
        let subscription = cx.subscribe(&sync_service, |this, _service, event: &ServiceEvent, cx| {
            match event {
                ServiceEvent::StatusChanged { running } => {
                    this.is_service_running = *running;
                    cx.notify();
                }
                _ => {}
            }
        });

        // 获取初始状态
        let initial_state = sync_service.read(cx).is_running();

        Self {
            sync_service,
            is_service_running: initial_state,
            _subscription: subscription,
        }
    }
}

impl Render for ZedVisionStatusButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_running = self.is_service_running;
        div().child(
            IconButton::new("zed-vision-status-button", IconName::Server)
                .icon_size(IconSize::Small)
                .icon_color(if is_running {
                    Color::Success
                } else {
                    Color::Muted
                })
                .tooltip(move |window, cx| {
                    Tooltip::for_action(
                        if is_running {
                            "Vision Pro Sync (Running)"
                        } else {
                            "Vision Pro Sync"
                        },
                        &ToggleFocus,
                        window,
                        cx,
                    )
                })
                .on_click(cx.listener(|_this, _, window, cx| {
                    window.dispatch_action(Box::new(ToggleFocus), cx);
                })),
        )
    }
}

impl StatusItemView for ZedVisionStatusButton {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // 这里可以根据需要更新状态
    }
}
