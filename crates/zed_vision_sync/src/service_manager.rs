use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;
use warp::Filter;
use serde_json;
use gpui::{Entity, WeakEntity};
use std::collections::HashMap;

use crate::{ZedVisionConfig, Connection, ZedVisionMessage, ServerInfo};
use agent::{ThreadStore, Thread, ThreadEvent, MessageId as AgentMessageId};
use assistant_tool::ToolWorkingSet;
use project::Project;
use prompt_store::PromptBuilder;
use workspace::Workspace;

/// AI 会话状态
#[derive(Debug)]
struct AISession {
    session_id: String,
    thread: Entity<Thread>,
    thread_store: Entity<ThreadStore>,
    event_receiver: mpsc::UnboundedReceiver<ThreadEvent>,
}

/// 现代化服务管理器：HTTP 发现 + WebSocket 连接 + 真实 AI 集成
pub struct ServiceManager {
    config: ZedVisionConfig,
    is_running: Arc<RwLock<bool>>,
    connections: Arc<RwLock<Vec<Connection>>>,
    http_handle: Option<tokio::task::JoinHandle<()>>,
    websocket_handle: Option<tokio::task::JoinHandle<()>>,

    // AI 集成组件
    workspace: Option<WeakEntity<Workspace>>,
    ai_sessions: Arc<RwLock<HashMap<String, AISession>>>,
}

impl ServiceManager {
    pub fn new_with_shared_connections(
        config: ZedVisionConfig,
        connections: Arc<RwLock<Vec<Connection>>>,
    ) -> Self {
        Self {
            config,
            is_running: Arc::new(RwLock::new(false)),
            connections,
            http_handle: None,
            websocket_handle: None,
            workspace: None,
            ai_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 设置 Workspace 引用以启用 AI 功能
    pub fn set_workspace(&mut self, workspace: WeakEntity<Workspace>) {
        self.workspace = Some(workspace);
        log::info!("🎯 AI integration enabled: Workspace reference set");
    }

    pub async fn start(&mut self) -> Result<()> {
        if *self.is_running.read().await {
            return Ok(());
        }

        log::info!("Starting ZedVision HTTP + WebSocket service");

        // 启动 HTTP 发现服务
        let http_handle = self.start_http_discovery().await?;
        self.http_handle = Some(http_handle);

        // 启动 WebSocket 服务器
        let websocket_handle = self.start_websocket_server().await?;
        self.websocket_handle = Some(websocket_handle);

        *self.is_running.write().await = true;
        log::info!("ZedVision service started: HTTP discovery on port {}, WebSocket on port {}",
                   self.config.port + 1, self.config.port);

        Ok(())
    }



    async fn start_http_discovery(&self) -> Result<tokio::task::JoinHandle<()>> {
        let port = self.config.port;
        let service_name = self.config.service_name.clone();

        let handle = tokio::spawn(async move {
            log::info!("Starting HTTP discovery service on port {}", port + 1);

            // 获取本地 IP 地址
            let local_ip = Self::get_local_ip().unwrap_or_else(|| "0.0.0.0".to_string());

            // 创建发现端点
            let discover = warp::path("discover")
                .and(warp::get())
                .map(move || {
                    let response = serde_json::json!({
                        "name": service_name,
                        "websocket_url": format!("ws://{}:{}", local_ip, port),
                        "version": "1.0",
                        "platform": "macOS",
                        "app": "Zed"
                    });
                    warp::reply::json(&response)
                });

            // CORS 支持
            let cors = warp::cors()
                .allow_any_origin()
                .allow_headers(vec!["content-type"])
                .allow_methods(vec!["GET"]);

            let routes = discover.with(cors);

            // 启动 HTTP 服务器
            log::info!("HTTP discovery service listening on 0.0.0.0:{}", port + 1);
            warp::serve(routes)
                .run(([0, 0, 0, 0], port + 1))
                .await;
        });

        Ok(handle)
    }

    async fn start_websocket_server(&self) -> Result<tokio::task::JoinHandle<()>> {
        use tokio::net::TcpListener;
        use tokio_tungstenite::{accept_async, tungstenite::Message};
        use futures_util::{SinkExt, StreamExt};

        let port = self.config.port;
        let connections = self.connections.clone();
        let ai_sessions = self.ai_sessions.clone();
        let workspace = self.workspace.clone();

        let handle = tokio::spawn(async move {
            log::info!("Starting WebSocket server on port {}", port);

            let listener = match TcpListener::bind(format!("0.0.0.0:{}", port)).await {
                Ok(listener) => listener,
                Err(e) => {
                    log::error!("Failed to bind WebSocket server: {}", e);
                    return;
                }
            };

            log::info!("WebSocket server listening on 0.0.0.0:{}", port);

            while let Ok((stream, addr)) = listener.accept().await {
                log::info!("New WebSocket connection from: {}", addr);

                let connections = connections.clone();
                let ai_sessions = ai_sessions.clone();
                let workspace = workspace.clone();

                tokio::spawn(async move {
                    let ws_stream = match accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            log::error!("WebSocket handshake failed: {}", e);
                            return;
                        }
                    };

                    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

                    // 创建连接记录
                    let mut connection = Connection {
                        id: Uuid::new_v4(),
                        device_name: format!("Device-{}", addr),
                        connected_at: std::time::SystemTime::now(),
                        last_activity: std::time::SystemTime::now(),
                    };

                    // 添加到连接列表
                    connections.write().await.push(connection.clone());
                    let connection_count = connections.read().await.len();
                    log::info!("Added connection: {} ({}) - Total connections: {}",
                              connection.device_name, connection.id, connection_count);

                    // 发送欢迎消息
                    let welcome_msg = ZedVisionMessage::ConnectionAccepted {
                        payload: crate::ConnectionAcceptedPayload {
                            connection_id: connection.id,
                            server_info: ServerInfo {
                                name: "Zed".to_string(),
                                version: "1.0".to_string(),
                                platform: "macOS".to_string(),
                            },
                        }
                    };

                    let welcome_json = serde_json::to_string(&welcome_msg).unwrap();
                    if let Err(e) = ws_sender.send(Message::Text(welcome_json)).await {
                        log::error!("Failed to send welcome message: {}", e);
                        return;
                    }

                    // 处理消息
                    while let Some(msg) = ws_receiver.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                log::info!("Received message: {}", text);

                                // 更新连接活动时间
                                connection.update_activity();

                                // 在连接列表中更新活动时间
                                {
                                    let mut connections_guard = connections.write().await;
                                    if let Some(conn) = connections_guard.iter_mut().find(|c| c.id == connection.id) {
                                        conn.update_activity();
                                    }
                                }

                                let response = if let Ok(msg) = serde_json::from_str::<ZedVisionMessage>(&text) {
                                    log::info!("✅ Successfully parsed JSON message: {:?}", msg);
                                    // 处理结构化消息
                                    match msg {
                                        ZedVisionMessage::Ping => {
                                            serde_json::to_string(&ZedVisionMessage::Pong).unwrap()
                                        }
                                        ZedVisionMessage::ClientHandshake { client_type, version, capabilities } => {
                                            log::info!("🤝 Received handshake from {} v{} with capabilities: {:?}",
                                                client_type, version, capabilities);

                                            // 发送连接接受响应
                                            serde_json::to_string(&ZedVisionMessage::ConnectionAccepted {
                                                payload: crate::ConnectionAcceptedPayload {
                                                    connection_id: connection.id,
                                                    server_info: crate::ServerInfo {
                                                        name: "Zed".to_string(),
                                                        version: "1.0".to_string(),
                                                        platform: "macOS".to_string(),
                                                    }
                                                }
                                            }).unwrap()
                                        }
                                        ZedVisionMessage::AIConversation { payload } => {
                                            // 处理 AI 对话消息 - 使用真实 AI 集成
                                            Self::handle_ai_conversation_static(
                                                payload.id,
                                                payload.session_id,
                                                payload.role,
                                                payload.content,
                                                payload.timestamp,
                                                ai_sessions.clone(),
                                                workspace.clone()
                                            ).await
                                        }
                                        _ => {
                                            // 回显其他消息
                                            serde_json::to_string(&ZedVisionMessage::Echo {
                                                payload: crate::EchoPayload {
                                                    original: serde_json::to_value(&msg).unwrap(),
                                                    timestamp: std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .unwrap()
                                                        .as_millis() as u64,
                                                }
                                            }).unwrap()
                                        }
                                    }
                                } else {
                                    log::warn!("❌ Received non-JSON message, ignoring: {}", text);
                                    // 优雅地忽略非 JSON 消息，不发送响应
                                    continue;
                                };

                                if let Err(e) = ws_sender.send(Message::Text(response)).await {
                                    log::error!("Failed to send response: {}", e);
                                    break;
                                }
                            }
                            Ok(Message::Close(_)) => {
                                log::info!("WebSocket connection closed by client");
                                break;
                            }
                            Err(e) => {
                                log::error!("WebSocket error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }

                    // 移除连接
                    connections.write().await.retain(|c| c.id != connection.id);
                    let connection_count = connections.read().await.len();
                    log::info!("Removed connection: {} ({}) - Total connections: {}",
                              connection.device_name, connection.id, connection_count);
                });
            }
        });

        Ok(handle)
    }

    /// 获取本地 IP 地址
    fn get_local_ip() -> Option<String> {
        use std::net::UdpSocket;

        // 尝试连接到外部地址来获取本地 IP
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    return Some(addr.ip().to_string());
                }
            }
        }
        None
    }

    /// 处理 AI 对话消息 - 静态方法用于 tokio::spawn 上下文
    async fn handle_ai_conversation_static(
        id: String,
        session_id: String,
        role: String,
        content: String,
        timestamp: u64,
        ai_sessions: Arc<RwLock<HashMap<String, AISession>>>,
        workspace: Option<WeakEntity<Workspace>>,
    ) -> String {
        log::info!("🤖 Processing real AI conversation: {} ({})", content, role);

        // 只处理用户消息
        if role != "user" {
            return Self::create_echo_response_static(id, session_id, role, content, timestamp);
        }

        // 检查是否有 Workspace 引用
        let workspace = match workspace {
            Some(workspace) => workspace,
            None => {
                log::warn!("❌ No workspace available for AI integration");
                return Self::create_error_response_static(session_id, "AI service not available".to_string());
            }
        };

        // 暂时返回占位响应，真实集成需要在 GPUI 上下文中进行
        log::info!("📝 AI integration placeholder for session: {}", session_id);
        Self::create_placeholder_response(session_id, content)
    }

    /// 创建占位响应（临时实现）
    fn create_placeholder_response(session_id: String, user_content: String) -> String {
        let response = ZedVisionMessage::AIConversation {
            payload: crate::AIConversationPayload {
                id: uuid::Uuid::new_v4().to_string(),
                session_id,
                role: "assistant".to_string(),
                content: format!("🔄 AI integration in progress. Your message: \"{}\" has been received and will be processed by Zed's AI system.", user_content),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            }
        };
        serde_json::to_string(&response).unwrap_or_else(|_| "Error generating response".to_string())
    }

    /// 获取或创建 AI 会话
    async fn get_or_create_ai_session(
        &self,
        session_id: &str,
        workspace: WeakEntity<Workspace>,
    ) -> Result<Entity<Thread>> {
        // 检查现有会话
        {
            let sessions = self.ai_sessions.read().await;
            if let Some(session) = sessions.get(session_id) {
                return Ok(session.thread.clone());
            }
        }

        // 创建新的 AI 会话
        log::info!("🆕 Creating new AI session: {}", session_id);

        // 这里需要在 GPUI 上下文中执行
        // 暂时返回错误，需要重构为在正确的上下文中创建
        Err(anyhow::anyhow!("AI session creation needs GPUI context"))
    }

    /// 发送消息到 AI Thread
    async fn send_to_ai_thread(
        &self,
        _thread: Entity<Thread>,
        _content: String,
        session_id: String,
    ) -> String {
        // 这里需要在 GPUI 上下文中执行 Thread::send()
        // 暂时返回占位响应
        log::info!("📤 Sending message to AI thread for session: {}", session_id);

        Self::create_error_response_static(session_id, "AI integration in progress".to_string())
    }

    /// 创建错误响应（静态方法）
    fn create_error_response_static(session_id: String, error: String) -> String {
        let response = ZedVisionMessage::AIConversation {
            payload: crate::AIConversationPayload {
                id: uuid::Uuid::new_v4().to_string(),
                session_id,
                role: "assistant".to_string(),
                content: format!("Error: {}", error),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            }
        };
        serde_json::to_string(&response).unwrap_or_else(|_| "Error generating response".to_string())
    }

    /// 创建回显响应（静态方法）
    fn create_echo_response_static(
        id: String,
        session_id: String,
        role: String,
        content: String,
        timestamp: u64,
    ) -> String {
        serde_json::to_string(&ZedVisionMessage::Echo {
            payload: crate::EchoPayload {
                original: serde_json::json!({
                    "id": id,
                    "session_id": session_id,
                    "role": role,
                    "content": content,
                    "timestamp": timestamp
                }),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            }
        }).unwrap_or_else(|_| "Error processing message".to_string())
    }


}
