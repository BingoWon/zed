use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
use warp::Filter;

use crate::{ZedVisionConfig, Connection, ZedVisionMessage, ServerInfo};

/// 现代化服务管理器：HTTP 发现 + WebSocket 连接
pub struct ServiceManager {
    config: ZedVisionConfig,
    is_running: Arc<RwLock<bool>>,
    connections: Arc<RwLock<Vec<Connection>>>,
    http_handle: Option<tokio::task::JoinHandle<()>>,
    websocket_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ServiceManager {


    pub fn new_with_shared_connections(config: ZedVisionConfig, connections: Arc<RwLock<Vec<Connection>>>) -> Self {
        Self {
            config,
            is_running: Arc::new(RwLock::new(false)),
            connections,
            http_handle: None,
            websocket_handle: None,
        }
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
                        connection_id: connection.id,
                        server_info: ServerInfo {
                            name: "Zed".to_string(),
                            version: "1.0".to_string(),
                            platform: "macOS".to_string(),
                        },
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

                                let response = if text.trim() == "hello" {
                                    // 简单的 hello 响应
                                    "hello from zed server".to_string()
                                } else if let Ok(msg) = serde_json::from_str::<ZedVisionMessage>(&text) {
                                    // 处理结构化消息
                                    match msg {
                                        ZedVisionMessage::Ping => {
                                            serde_json::to_string(&ZedVisionMessage::Pong).unwrap()
                                        }

                                        _ => {
                                            // 回显其他消息
                                            serde_json::to_string(&ZedVisionMessage::Echo {
                                                original: serde_json::to_value(&msg).unwrap(),
                                                timestamp: std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .unwrap()
                                                    .as_millis() as u64,
                                            }).unwrap()
                                        }
                                    }
                                } else {
                                    // 处理非结构化文本
                                    format!("received: {}", text)
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
}
