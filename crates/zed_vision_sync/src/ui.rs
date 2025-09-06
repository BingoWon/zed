use gpui::{
    div, App, Entity, EventEmitter, FocusHandle, IntoElement,
    ParentElement, Render, Window, Context, Focusable, actions, Subscription, FontWeight, Task,
};
use ui::{prelude::*, Button, Icon, IconName, Label};
use workspace::dock::{Panel, DockPosition, PanelEvent};
use std::net::Ipv4Addr;

use crate::{ZedVisionSync, ServiceEvent};

actions!(zed_vision_sync, [ToggleFocus]);

pub struct VisionSyncPanel {
    focus_handle: FocusHandle,
    sync_service: Entity<ZedVisionSync>,
    is_service_running: bool,
    network_info: NetworkInfo,
    _subscription: Subscription,
    _refresh_task: Task<()>,
    panel_width: f32, // 动态面板宽度
}

#[derive(Clone, Debug)]
struct NetworkInfo {
    local_ip: String,
    network_segment: String,
    interface_name: String,
}

impl VisionSyncPanel {
    pub fn new(sync_service: Entity<ZedVisionSync>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        // 订阅服务状态变化
        let subscription = cx.subscribe(&sync_service, |this, _service, event: &ServiceEvent, cx| {
            match event {
                ServiceEvent::StatusChanged { running } => {
                    this.is_service_running = *running;
                    cx.notify();
                }
                ServiceEvent::ConnectionAdded { .. } | ServiceEvent::ConnectionRemoved { .. } => {
                    cx.notify();
                }
            }
        });

        // 获取初始状态
        let initial_state = sync_service.read(cx).is_running();

        // 获取网络信息
        let network_info = Self::get_network_info();

        // 启动定期刷新任务
        let refresh_task = Self::start_refresh_task(sync_service.clone(), cx);

        Self {
            focus_handle,
            sync_service,
            is_service_running: initial_state,
            network_info,
            _subscription: subscription,
            _refresh_task: refresh_task,
            panel_width: 350.0, // 默认宽度，支持动态调整
        }
    }

    fn get_network_info() -> NetworkInfo {
        // 获取本地网络信息
        let local_ip = Self::get_local_ip().unwrap_or_else(|| "Unknown".to_string());
        let network_segment = Self::calculate_network_segment(&local_ip);
        let interface_name = Self::get_primary_interface().unwrap_or_else(|| "Unknown".to_string());

        NetworkInfo {
            local_ip,
            network_segment,
            interface_name,
        }
    }

    fn get_local_ip() -> Option<String> {
        use std::net::UdpSocket;

        // 尝试连接到外部地址来获取本地 IP
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if let Ok(_) = socket.connect("8.8.8.8:80") {
                if let Ok(addr) = socket.local_addr() {
                    return Some(addr.ip().to_string());
                }
            }
        }
        None
    }

    fn calculate_network_segment(ip: &str) -> String {
        if let Ok(addr) = ip.parse::<Ipv4Addr>() {
            let octets = addr.octets();
            format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2])
        } else {
            "Unknown".to_string()
        }
    }

    fn get_primary_interface() -> Option<String> {
        // 简化实现，返回默认接口名
        Some("en0".to_string())
    }

    fn toggle_service(&mut self, cx: &mut Context<Self>) {
        log::info!("Toggling ZedVision service: {} -> {}", self.is_service_running, !self.is_service_running);

        self.sync_service.update(cx, |service, cx| {
            service.toggle_service(cx);
        });
    }

    fn start_refresh_task(_sync_service: Entity<ZedVisionSync>, cx: &mut Context<Self>) -> Task<()> {
        use std::time::Duration;

        let _handle = cx.entity().downgrade();
        cx.spawn(async move |_panel, cx| {
            loop {
                // 每 1 秒刷新一次 UI，提高响应性
                cx.background_executor().timer(Duration::from_secs(1)).await;

                if let Some(panel) = _handle.upgrade() {
                    panel.update(cx, |_panel, cx| {
                        // 触发 UI 重新渲染
                        cx.notify();
                    }).ok();
                } else {
                    // Panel 已被销毁，退出循环
                    break;
                }
            }
        })
    }
}

impl Render for VisionSyncPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(Label::new("Vision Pro Sync"))
                    .child(
                        Button::new("toggle_service", if self.is_service_running { "Stop" } else { "Start" })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_service(cx);
                            }))
                    )
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(if self.is_service_running {
                            IconName::Check
                        } else {
                            IconName::Close
                        })
                        .color(if self.is_service_running {
                            Color::Success
                        } else {
                            Color::Muted
                        })
                    )
                    .child(Label::new(if self.is_service_running {
                        format!("Service Running - {}", self.sync_service.read(cx).get_config().service_name)
                    } else {
                        "Service Stopped".to_string()
                    }))
            )
            .when(self.is_service_running, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Server).color(Color::Accent))
                        .child(Label::new(format!("Port: {}", self.sync_service.read(cx).get_config().port)))
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .bg(cx.theme().colors().element_background)
                    .rounded_md()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::Server).color(Color::Info))
                            .child(Label::new("Network Information").weight(FontWeight::MEDIUM))
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Label::new("Local IP:").color(Color::Muted))
                            .child(Label::new(&self.network_info.local_ip))
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Label::new("Network Segment:").color(Color::Muted))
                            .child(Label::new(&self.network_info.network_segment))
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Label::new("Interface:").color(Color::Muted))
                            .child(Label::new(&self.network_info.interface_name))
                    )
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Label::new(format!("Connected Devices ({})", self.sync_service.read(cx).get_connections().len())))
                    .children(
                        self.sync_service.read(cx).get_connections().iter().map(|conn| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .p_2()
                                .bg(cx.theme().colors().surface_background)
                                .rounded_md()
                                .child(Icon::new(IconName::Person).color(
                                    if conn.is_timed_out() { Color::Error } else { Color::Success }
                                ))
                                .child(Label::new(&conn.device_name))
                                .child(Label::new(
                                    format!("Last activity: {}s ago",
                                        conn.last_activity.elapsed().unwrap_or_default().as_secs()
                                    )
                                ).color(if conn.is_timed_out() { Color::Error } else { Color::Muted }))
                        })
                    )
            )
            .when(self.sync_service.read(cx).get_connections().is_empty() && self.is_service_running, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .child(Label::new("Waiting for Vision Pro connections...").color(Color::Muted))
                )
            })
    }
}

impl Panel for VisionSyncPanel {
    fn persistent_name() -> &'static str {
        "VisionSyncPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn set_position(&mut self, _position: DockPosition, _window: &mut Window, _cx: &mut Context<Self>) {
        // 位置设置逻辑
    }

    fn size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        gpui::px(self.panel_width)
    }

    fn set_size(&mut self, size: Option<gpui::Pixels>, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(size) = size {
            // 限制最小和最大宽度
            self.panel_width = size.0.clamp(250.0, 800.0);
            cx.notify(); // 通知 UI 更新
        }
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Server)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Vision Pro Sync")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        1
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right | DockPosition::Left)
    }

    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {
        // 状态通过事件订阅自动更新
    }
}

impl EventEmitter<PanelEvent> for VisionSyncPanel {}

impl Focusable for VisionSyncPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub fn init(_cx: &mut App) {
    // Action 注册将在工作空间级别处理
}
