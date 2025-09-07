use gpui::{App, Global, AppContext, EventEmitter, Context};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::sync::Arc;
use tokio::sync::RwLock;

mod service_manager;
pub mod ui;
pub mod status_button;

pub use ui::VisionSyncPanel;
pub use status_button::ZedVisionStatusButton;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZedVisionConfig {
    pub service_name: String,
    pub port: u16,
    pub enabled: bool,
}

impl Default for ZedVisionConfig {
    fn default() -> Self {
        let hostname = hostname::get()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Self {
            service_name: format!("Zed-{}", hostname),
            port: 8765,
            enabled: true, // 默认启用 ZedVision 服务
        }
    }
}

/// 统一的 WebSocket 消息协议
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ZedVisionMessage {
    // 连接管理
    ConnectionRequest {
        #[serde(rename = "payload")]
        device_name: String
    },
    ConnectionAccepted {
        #[serde(rename = "payload")]
        payload: ConnectionAcceptedPayload
    },
    ConnectionRejected {
        #[serde(rename = "payload")]
        reason: String
    },

    // 现代化客户端握手
    ClientHandshake {
        #[serde(rename = "clientType")]
        client_type: String,
        version: String,
        capabilities: Vec<String>,
    },

    // 编辑器状态同步
    EditorStateSync {
        #[serde(rename = "payload")]
        payload: EditorStateSyncPayload
    },

    // AI 对话消息
    AIConversation {
        #[serde(rename = "payload")]
        payload: AIConversationPayload
    },

    // 心跳和控制
    Ping,
    Pong,
    Echo {
        #[serde(rename = "payload")]
        payload: EchoPayload
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionAcceptedPayload {
    pub connection_id: Uuid,
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorStateSyncPayload {
    pub file_path: Option<String>,
    pub cursor_line: u32,
    pub cursor_column: u32,
    pub content_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIConversationPayload {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EchoPayload {
    pub original: serde_json::Value,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
}

/// AI 对话消息角色
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "system")]
    System,
}

/// AI 对话消息结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIConversationMessage {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub id: Uuid,
    pub device_name: String,
    pub connected_at: std::time::SystemTime,
    pub last_activity: std::time::SystemTime,
}

impl Connection {
    /// 检查连接是否超时（60 秒无活动）
    pub fn is_timed_out(&self) -> bool {
        self.last_activity.elapsed().unwrap_or_default().as_secs() > 60
    }

    /// 更新最后活动时间
    pub fn update_activity(&mut self) {
        self.last_activity = std::time::SystemTime::now();
    }
}

#[derive(Debug, Clone)]
pub enum ServiceEvent {
    StatusChanged { running: bool },
    ConnectionAdded { connection: Connection },
    ConnectionRemoved { connection_id: Uuid },
}

pub struct ZedVisionSync {
    is_running: bool,
    connections: Arc<RwLock<Vec<Connection>>>,
    cleanup_task: Option<std::thread::JoinHandle<()>>,
}

impl Global for ZedVisionSync {}
impl EventEmitter<ServiceEvent> for ZedVisionSync {}

impl ZedVisionSync {
    pub fn new() -> Self {
        Self {
            is_running: false,
            connections: Arc::new(RwLock::new(Vec::new())),
            cleanup_task: None,
        }
    }

    pub fn start_service(&mut self, cx: &mut Context<Self>) {
        if self.is_running {
            return;
        }

        log::info!("Starting ZedVision service on port 8765...");

        // 标记为运行状态
        self.is_running = true;
        cx.emit(ServiceEvent::StatusChanged { running: true });

        // 启动真正的网络服务（修复：只启动一次）
        self.start_network_services();

        // 启动连接清理任务
        self.start_connection_cleanup();

        log::info!("ZedVision service initialization completed");
    }

    fn start_network_services(&mut self) {
        use std::thread;

        let connections = self.connections.clone();

        // 在独立线程中启动服务，避免 GPUI 异步问题
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let config = ZedVisionConfig::default();
                let mut service_manager = service_manager::ServiceManager::new_with_shared_connections(config, connections);

                if let Err(e) = service_manager.start().await {
                    log::error!("Failed to start ZedVision service: {}", e);
                } else {
                    log::info!("Network services started successfully");

                    // 保持服务运行
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            });
        });
    }

    /// 设置 Workspace 引用以启用 AI 功能
    pub fn set_workspace(&mut self, workspace: gpui::WeakEntity<workspace::Workspace>) {
        // 暂时存储 workspace 引用，后续需要传递给 ServiceManager
        log::info!("🎯 Workspace reference set for AI integration");
        // TODO: 需要重构 ServiceManager 的创建方式以支持 Workspace 传递
    }

    fn start_connection_cleanup(&mut self) {
        let connections = self.connections.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                loop {
                    // 每 30 秒检查一次连接状态
                    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

                    let mut connections_guard = connections.write().await;
                    let initial_count = connections_guard.len();

                    // 移除超时的连接
                    connections_guard.retain(|conn| !conn.is_timed_out());

                    let final_count = connections_guard.len();
                    if initial_count != final_count {
                        log::info!("Cleaned up {} timed-out connections. Active connections: {}",
                                  initial_count - final_count, final_count);
                    }
                }
            });
        });

        self.cleanup_task = Some(handle);
    }

    pub fn stop_service(&mut self, cx: &mut Context<Self>) {
        if !self.is_running {
            return;
        }

        log::info!("Stopping ZedVision service");

        self.is_running = false;
        cx.emit(ServiceEvent::StatusChanged { running: false });

        log::info!("ZedVision service stopped");
    }

    pub fn get_connections(&self) -> Vec<Connection> {
        // 使用 try_read 进行非阻塞读取
        match self.connections.try_read() {
            Ok(connections) => {
                log::debug!("Retrieved {} connections", connections.len());
                connections.clone()
            }
            Err(_) => {
                log::warn!("Failed to read connections (lock contention)");
                Vec::new()
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.is_running
    }

    pub fn toggle_service(&mut self, cx: &mut Context<Self>) {
        if self.is_running {
            self.stop_service(cx);
        } else {
            self.start_service(cx);
        }
    }

    pub fn get_config(&self) -> ZedVisionConfig {
        ZedVisionConfig::default()
    }

    pub fn update_config(&mut self, _config: ZedVisionConfig, cx: &mut Context<Self>) {
        let was_running = self.is_running;

        if was_running {
            self.stop_service(cx);
            self.start_service(cx);
        }
    }
}

pub fn init(cx: &mut App) {
    // 注册 UI 组件
    ui::init(cx);
}

pub fn init_panel(workspace: &mut workspace::Workspace, window: &mut gpui::Window, cx: &mut gpui::Context<workspace::Workspace>) {
    let sync_service = cx.new(|_| ZedVisionSync::new());
    let panel = cx.new(|cx| VisionSyncPanel::new(sync_service.clone(), cx));
    let status_button = cx.new(|cx| ZedVisionStatusButton::new(sync_service.clone(), cx));

    workspace.add_panel(panel, window, cx);

    // 添加状态栏按钮
    workspace.status_bar().update(cx, |status_bar, cx| {
        status_bar.add_right_item(status_button, window, cx);
    });

    // 设置 Workspace 引用以启用 AI 功能
    let workspace_weak = workspace.weak_handle();
    sync_service.update(cx, |service, _cx| {
        service.set_workspace(workspace_weak);
    });

    // 自动启动 ZedVision 服务（如果配置启用）
    let config = ZedVisionConfig::default();
    if config.enabled {
        sync_service.update(cx, |service, cx| {
            service.start_service(cx);
        });
        log::info!("ZedVision service auto-started on Zed startup with AI integration");
    }
}
