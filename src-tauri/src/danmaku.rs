//! B 站直播弹幕接入：每个排队队列一个独立的长连接任务。
//!
//! 流程（参考 B 站网页播放器的弹幕协议）：
//! 1. `room/v1/Room/get_info` 把短号解析成真实房间号；
//! 2. `room/v1/Danmu/getConf` 拿到弹幕服务器地址与鉴权 token；
//! 3. `wss://host:port/sub` 建立 WebSocket，发送 op=7 鉴权包；
//! 4. 服务端推送 op=5 通知包（protover=2 时正文是 zlib 压缩的嵌套包）；
//! 5. 每 30 秒发一次心跳。
//!
//! 队列的「轮询间隔」不是去轮询 HTTP，而是**批量写入 / 界面刷新间隔**：
//! 弹幕仍然实时到达，命中正则后先攒在内存里，每到间隔统一写 SQLite 并广播，
//! 避免一条弹幕一次全量广播把主窗口刷爆。
//!
//! 网络依赖全部 gate 到 `cfg(windows)`（与 updater 一致），Linux 原生构建
//! 只编译空实现，不影响 CNB 的交叉编译链路。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// 队列弹幕连接状态（只在内存里，不写数据库；主窗口定时查询）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueStatus {
    pub queue_id: i64,
    pub connected: bool,
    /// 解析后的真实房间号（短号会被换成它）
    pub real_room_id: i64,
    /// 最近收到任意弹幕的时间（毫秒时间戳；只用于展示连接活跃度）
    pub last_message_at: i64,
    pub last_error: String,
    pub updated_at: i64,
}

/// 按数据库里的最新队列配置启动 / 停止 / 重启弹幕任务。
/// 命令层在队列增删改后调用；启动时也会调用一次。
pub fn reconcile(app: &AppHandle) {
    let store = app.state::<Arc<crate::store::Store>>();
    let queues = store.get().queues;
    let manager = app.state::<DanmakuManager>();
    manager.reconcile(app, &queues);
}

#[cfg(windows)]
mod imp {
    use std::collections::{HashMap, VecDeque};
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use tauri::{AppHandle, Emitter, Manager};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    use super::QueueStatus;
    use crate::store::{now_ms, QueueConfig, Store};

    const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    const REFERER: &str = "https://live.bilibili.com/";
    const ROOM_INFO_URL: &str = "https://api.live.bilibili.com/room/v1/Room/get_info?room_id=";
    const DANMU_CONF_URL: &str =
        "https://api.live.bilibili.com/room/v1/Danmu/getConf?platform=pc&player=web&room_id=";
    const BUVID_URL: &str = "https://api.bilibili.com/x/frontend/finger/spi";

    struct WorkerHandle {
        signature: String,
        cancel: Arc<AtomicBool>,
    }

    /// 弹幕任务管理器（Tauri 状态）：管取消标志与队列连接状态。
    #[derive(Default)]
    pub struct DanmakuManager {
        workers: Mutex<HashMap<i64, WorkerHandle>>,
        status: Mutex<HashMap<i64, QueueStatus>>,
    }

    impl DanmakuManager {
        pub fn reconcile(&self, app: &AppHandle, queues: &[QueueConfig]) {
            let mut desired: HashMap<i64, String> = HashMap::new();
            for q in queues {
                if q.enabled && q.room_id > 0 {
                    desired.insert(q.id, format!("{}:{}", q.room_id, q.poll_secs));
                }
            }
            log::info!(
                "[弹幕] reconcile：数据库队列 {} 个，需监听 {} 个（enabled 且有直播间号）",
                queues.len(),
                desired.len()
            );

            // 一次持有 workers 锁，原子完成「停旧 + 占位新」。
            // 旧实现把 insert 放在释放锁之后，两次并发 reconcile 会给同一队列
            // 启动两个连接（多队列 + 同一直播间时更容易暴露），导致重复计数、
            // 互相抢 token / 被服务端踢下线。占位与检查必须在同一临界区。
            let mut spawned: Vec<(QueueConfig, Arc<AtomicBool>)> = Vec::new();
            match self.workers.lock() {
                Ok(mut workers) => {
                    for (id, w) in workers.iter() {
                        if desired.get(id) != Some(&w.signature) {
                            log::info!(
                                "[弹幕] reconcile：停止队列 #{id} 的监听（配置变化 / 暂停 / 删除）"
                            );
                            w.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    workers.retain(|id, w| desired.get(id) == Some(&w.signature));
                    for q in queues {
                        let Some(signature) = desired.get(&q.id) else {
                            continue;
                        };
                        if workers.contains_key(&q.id) {
                            continue;
                        }
                        let cancel = Arc::new(AtomicBool::new(false));
                        workers.insert(
                            q.id,
                            WorkerHandle {
                                signature: signature.clone(),
                                cancel: cancel.clone(),
                            },
                        );
                        log::info!(
                            "[弹幕] reconcile：为队列 #{} room={} 启动监听任务",
                            q.id,
                            q.room_id
                        );
                        spawned.push((q.clone(), cancel));
                    }
                }
                Err(_) => return,
            }

            // 状态表：删除已不存在的队列；未启用的显示原因；刚启动的清空旧错误
            if let Ok(mut status) = self.status.lock() {
                status.retain(|id, _| queues.iter().any(|q| q.id == *id));
                for q in queues {
                    let just_spawned = spawned.iter().any(|(s, _)| s.id == q.id);
                    if !desired.contains_key(&q.id) {
                        let s = status
                            .entry(q.id)
                            .or_insert_with(|| QueueStatus {
                                queue_id: q.id,
                                ..Default::default()
                            });
                        s.connected = false;
                        s.last_error = if q.room_id <= 0 {
                            log::info!(
                                "[弹幕] reconcile：队列 #{} room={} 未启动（未设置直播间号）",
                                q.id,
                                q.room_id
                            );
                            "未设置直播间号".into()
                        } else {
                            log::info!(
                                "[弹幕] reconcile：队列 #{} room={} 未启动（已暂停监听）",
                                q.id,
                                q.room_id
                            );
                            "已暂停监听".into()
                        };
                        if q.room_id <= 0 {
                            s.real_room_id = 0;
                        }
                        s.updated_at = now_ms();
                    } else if just_spawned {
                        let s = status
                            .entry(q.id)
                            .or_insert_with(|| QueueStatus {
                                queue_id: q.id,
                                ..Default::default()
                            });
                        s.connected = false;
                        s.last_error.clear();
                        s.real_room_id = 0;
                        s.updated_at = now_ms();
                    }
                }
            }

            for (q, cancel) in spawned {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    run_worker(app, q, cancel).await;
                });
            }
        }

        pub fn statuses(&self) -> Vec<QueueStatus> {
            let mut out: Vec<QueueStatus> = self
                .status
                .lock()
                .map(|s| s.values().cloned().collect())
                .unwrap_or_default();
            out.sort_by_key(|s| s.queue_id);
            out
        }

        fn patch(&self, queue_id: i64, f: impl FnOnce(&mut QueueStatus)) {
            if let Ok(mut status) = self.status.lock() {
                let s = status.entry(queue_id).or_insert_with(|| QueueStatus {
                    queue_id,
                    ..Default::default()
                });
                f(s);
                s.updated_at = now_ms();
            }
        }
    }

    async fn run_worker(app: AppHandle, queue: QueueConfig, cancel: Arc<AtomicBool>) {
        let queue_id = queue.id;
        log::info!(
            "[弹幕] queue_id={queue_id} room={} name={:?} 监听任务启动，刷新间隔={}s，规则数={}",
            queue.room_id,
            queue.name,
            queue.poll_secs,
            queue.patterns.len()
        );

        // 客户端初始化失败也不能直接结束任务：workers 表仍保留句柄，直接 return
        // 会让这个队列永远不再重连，暂停/恢复之外的开关看起来也像“卡死”。
        let client = loop {
            match reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(12))
                .build()
            {
                Ok(client) => break client,
                Err(e) => {
                    let error = format!("初始化网络失败：{e}");
                    log::error!("[弹幕] queue_id={queue_id} room={} {error}；10 秒后重试", queue.room_id);
                    set_error(&app, queue_id, error);
                    if wait_for_retry(&cancel, Duration::from_secs(10)).await {
                        patch_status(&app, queue_id, |s| s.connected = false);
                        log::info!("[弹幕] queue_id={queue_id} 监听任务已停止");
                        return;
                    }
                }
            }
        };

        let mut host_turn = 0usize;
        let mut attempt = 0u64;
        // 跨连接保留待写入弹幕：旧实现把它放在 stream_once 内，连接在两次刷新
        // 之间断开时，本批命中会随局部变量一起丢失。
        let mut pending: Vec<(i64, String, i64)> = Vec::new();
        while !cancel.load(Ordering::Relaxed) {
            attempt += 1;
            patch_status(&app, queue_id, |s| {
                s.connected = false;
                s.last_error.clear();
            });
            log::info!(
                "[弹幕] queue_id={queue_id} room={} 开始第 {attempt} 次连接，待写入={} ",
                queue.room_id,
                pending.len()
            );
            let backoff = match stream_once(
                &app,
                &client,
                &queue,
                &mut host_turn,
                &cancel,
                &mut pending,
            )
            .await
            {
                Ok(()) => {
                    log::info!("[弹幕] queue_id={queue_id} room={} 本次连接结束", queue.room_id);
                    Duration::from_secs(3)
                }
                Err(e) => {
                    log::error!(
                        "[弹幕] queue_id={queue_id} room={} 连接失败：{e}；6 秒后重连，保留待写入={} ",
                        queue.room_id,
                        pending.len()
                    );
                    set_error(&app, queue_id, e);
                    Duration::from_secs(6)
                }
            };
            if wait_for_retry(&cancel, backoff).await {
                break;
            }
        }
        patch_status(&app, queue_id, |s| s.connected = false);
        log::info!("[弹幕] queue_id={queue_id} room={} 监听任务已停止", queue.room_id);
    }

    /// 分片等待重连；返回 true 表示期间收到取消，可尽快结束任务。
    async fn wait_for_retry(cancel: &Arc<AtomicBool>, duration: Duration) -> bool {
        let until = tokio::time::Instant::now() + duration;
        while tokio::time::Instant::now() < until {
            if cancel.load(Ordering::Relaxed) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        cancel.load(Ordering::Relaxed)
    }

    /// 连接一次并持续接收；连接断开时返回错误，由 run_worker 决定重连。
    async fn stream_once(
        app: &AppHandle,
        client: &reqwest::Client,
        queue: &QueueConfig,
        host_turn: &mut usize,
        cancel: &Arc<AtomicBool>,
        pending: &mut Vec<(i64, String, i64)>,
    ) -> Result<(), String> {
        let queue_id = queue.id;

        // 1. 短号 → 真实房间号
        let t0 = std::time::Instant::now();
        log::info!(
            "队列 #{queue_id} 房间 {}：开始连接 B 站弹幕，解析真实房间号",
            queue.room_id
        );
        let info = get_json(client, &format!("{ROOM_INFO_URL}{}", queue.room_id)).await?;
        let code = info.get("code").and_then(as_i64).unwrap_or(-1);
        if code != 0 {
            let msg = info
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| info.get("msg").and_then(|v| v.as_str()))
                .unwrap_or("未知错误");
            return Err(format!("直播间 {}：{}", queue.room_id, msg));
        }
        let real_room_id = info
            .get("data")
            .and_then(|d| d.get("room_id"))
            .and_then(as_i64)
            .unwrap_or(0);
        if real_room_id <= 0 {
            return Err("没有解析到真实房间号，请检查直播间号".into());
        }
        patch_status(app, queue_id, |s| s.real_room_id = real_room_id);
        log::info!(
            "队列 #{queue_id} 房间 {} → 真实房间 {real_room_id}：解析完成，耗时 {}ms",
            queue.room_id,
            t0.elapsed().as_millis()
        );

        // 2. 弹幕服务器与 token
        let conf = get_json(client, &format!("{DANMU_CONF_URL}{real_room_id}")).await?;
        let conf_code = conf.get("code").and_then(as_i64).unwrap_or(-1);
        if conf_code != 0 {
            return Err(format!("获取弹幕服务器失败（{conf_code}）"));
        }
        let data = conf.get("data").ok_or("弹幕服务器配置为空")?;
        let token = data
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let mut hosts: Vec<(String, u16)> = Vec::new();
        if let Some(list) = data.get("host_server_list").and_then(|v| v.as_array()) {
            for item in list {
                let Some(host) = item.get("host").and_then(|v| v.as_str()) else {
                    continue;
                };
                let port = item
                    .get("wss_port")
                    .and_then(as_i64)
                    .or_else(|| item.get("port").and_then(as_i64))
                    .unwrap_or(443);
                let pair = (host.to_string(), port.clamp(1, 65535) as u16);
                if !hosts.contains(&pair) {
                    hosts.push(pair);
                }
            }
        }
        if hosts.is_empty() {
            hosts.push(("broadcastlv.chat.bilibili.com".into(), 443));
        }
        log::debug!(
            "队列 #{queue_id}：获取到 {} 个弹幕服务器",
            hosts.len()
        );
        // 3. 匿名弹幕也建议带 buvid，减少被风控拒绝的概率
        let buvid = fetch_buvid(client).await;

        let (host, port) = hosts[*host_turn % hosts.len()].clone();
        *host_turn = host_turn.wrapping_add(1);
        let url = format!("wss://{host}:{port}/sub");
        let mut request = url
            .into_client_request()
            .map_err(|e| format!("弹幕地址不合法：{e}"))?;
        {
            use tokio_tungstenite::tungstenite::http::HeaderValue;
            let headers = request.headers_mut();
            headers.insert("Origin", HeaderValue::from_static(REFERER.trim_end_matches('/')));
            headers.insert("Referer", HeaderValue::from_static(REFERER));
            headers.insert("User-Agent", HeaderValue::from_static(USER_AGENT));
            if let Some(buvid) = buvid.as_deref() {
                if let Ok(value) = HeaderValue::from_str(&format!("buvid3={buvid}")) {
                    headers.insert("Cookie", value);
                }
            }
        }
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(12),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| "连接弹幕服务器超时".to_string())?
        .map_err(|e| format!("连接弹幕服务器失败：{e}"))?;

        // 4. 鉴权
        let auth = serde_json::json!({
            "uid": 0,
            "roomid": real_room_id,
            "protover": 2,
            "platform": "web",
            "type": 2,
            "key": token,
            "buvid": buvid.unwrap_or_default(),
        });
        ws.send(Message::Binary(make_packet(7, auth.to_string().as_bytes()).into()))
            .await
            .map_err(|e| format!("发送弹幕鉴权失败：{e}"))?;
        log::info!(
            "队列 #{queue_id}：连接 {host}:{port} 成功，已发送鉴权包，耗时 {}ms",
            t0.elapsed().as_millis()
        );

        let mut patterns = compile_patterns(&queue.patterns);
        let mut seen: VecDeque<(String, std::time::Instant)> = VecDeque::new();
        let mut connected = false;
        let mut last_message_written = 0i64;
        let mut received_frames = 0u64;
        let mut received_danmaku = 0u64;
        let mut matched = 0u64;
        let mut duplicates = 0u64;
        let mut written = 0u64;

        let flush_secs = queue.poll_secs.clamp(1, 60) as u64;
        let mut flush_tick = tokio::time::interval(Duration::from_secs(flush_secs));
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        flush_tick.tick().await; // 第一次 tick 立即返回，先吃掉

        let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        heartbeat.tick().await;

        // 即使一直没有命中，也每分钟留下一条活跃度日志，便于区分
        // “连接正常但没匹配”与“连接已悄悄停住”。
        let mut stats_tick = tokio::time::interval(Duration::from_secs(60));
        stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        stats_tick.tick().await;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            tokio::select! {
                _ = heartbeat.tick() => {
                    ws.send(Message::Binary(make_packet(2, b"[object Object]").into()))
                        .await
                        .map_err(|e| format!("发送弹幕心跳失败：{e}"))?;
                    log::debug!("队列 #{queue_id}：已发送心跳");
                }
                _ = flush_tick.tick() => {
                    // 规则可能刚被修改：每个刷新周期读一次最新配置
                    if let Some(latest) = app
                        .state::<Arc<Store>>()
                        .get_queue(queue_id)
                    {
                        if !latest.enabled {
                            log::info!("[弹幕] queue_id={queue_id} 队列已暂停，结束当前连接");
                            return Ok(());
                        }
                        patterns = compile_patterns(&latest.patterns);
                    }
                    if !pending.is_empty() {
                        let hits = std::mem::take(pending);
                        let store = app.state::<Arc<Store>>();
                        match store.record_queue_matches(queue_id, &hits) {
                            Ok(n) if n > 0 => {
                                written += n as u64;
                                log::info!(
                                    "[弹幕] queue_id={queue_id} room={real_room_id} 本批写入={n}，累计写入={written}"
                                );
                                schedule_broadcast(app);
                            }
                            Ok(_) => {
                                log::debug!("[弹幕] queue_id={queue_id} 队列已删除，丢弃本批 {} 条命中", hits.len());
                            }
                            Err(e) => {
                                // 数据库暂时失败不能吃掉整批弹幕：恢复旧批次并保留
                                // 写入期间新到的弹幕，下一个刷新周期一起重试。
                                log::error!(
                                    "[弹幕] queue_id={queue_id} 写入 SQLite 失败：{e}；{} 条命中将在下周期重试",
                                    hits.len()
                                );
                                let mut retry = hits;
                                retry.append(pending);
                                *pending = retry;
                            }
                        }
                    }
                }
                _ = stats_tick.tick() => {
                    log::info!(
                        "[弹幕] queue_id={queue_id} room={real_room_id} 活跃统计：收包={received_frames}，弹幕={received_danmaku}，命中={matched}，重复={duplicates}，已写入={written}，待写入={}",
                        pending.len()
                    );
                }
                msg = tokio::time::timeout(Duration::from_secs(75), ws.next()) => {
                    let msg = msg.map_err(|_| "弹幕服务器长时间无响应".to_string())?;
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            received_frames += 1;
                            log::trace!(
                                "[弹幕] queue_id={queue_id} room={real_room_id} 收到二进制帧，字节数={}",
                                data.len()
                            );
                            let mut decoded = Vec::new();
                            if let Err(e) = decode_frame(&data, &mut decoded) {
                                return Err(format!("解析弹幕包失败：{e}"));
                            }
                            for (op, value) in decoded {
                                if op == 8 {
                                    let code = value.get("code").and_then(as_i64).unwrap_or(-1);
                                    if code != 0 {
                                        return Err(format!("弹幕鉴权失败（code {code}）"));
                                    }
                                    connected = true;
                                    patch_status(app, queue_id, |s| {
                                        s.connected = true;
                                        s.last_error.clear();
                                    });
                                    log::info!("[弹幕] queue_id={queue_id} room={real_room_id} 鉴权成功，开始接收消息");
                                    continue;
                                }
                                if op == 3 {
                                    log::trace!("[弹幕] queue_id={queue_id} room={real_room_id} 收到服务端心跳响应");
                                    continue;
                                }
                                if op != 5 {
                                    continue;
                                }
                                if !connected {
                                    connected = true;
                                    patch_status(app, queue_id, |s| {
                                        s.connected = true;
                                        s.last_error.clear();
                                    });
                                }
                                let Some((uid, nickname, text, at)) = parse_danmaku(&value) else {
                                    continue;
                                };
                                received_danmaku += 1;
                                let now = now_ms();
                                if now - last_message_written >= 1000 {
                                    last_message_written = now;
                                    patch_status(app, queue_id, |s| s.last_message_at = now);
                                }
                                if uid <= 0 || nickname.trim().is_empty() {
                                    continue;
                                }
                                if !patterns.iter().any(|re| re.is_match(&text)) {
                                    continue;
                                }
                                let key = format!("{uid}:{at}:{text}");
                                if is_duplicate(&mut seen, key) {
                                    duplicates += 1;
                                    log::debug!("[弹幕] queue_id={queue_id} 忽略重复弹幕：uid={uid} at={at} text={text:?}");
                                    continue;
                                }
                                matched += 1;
                                log::debug!(
                                    "[弹幕] queue_id={queue_id} room={real_room_id} 命中：uid={uid} nickname={nickname:?} text={text:?}，待写入={}",
                                    pending.len() + 1
                                );
                                pending.push((uid, nickname, at));
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            ws.send(Message::Pong(payload))
                                .await
                                .map_err(|e| format!("回复 WebSocket Ping 失败：{e}"))?;
                            log::trace!("[弹幕] queue_id={queue_id} 已回复 WebSocket Pong");
                        }
                        Some(Ok(Message::Close(frame))) => {
                            log::warn!("[弹幕] queue_id={queue_id} room={real_room_id} 服务端关闭连接：{frame:?}");
                            return Err("弹幕服务器关闭了连接".into());
                        }
                        Some(Ok(other)) => {
                            log::debug!("[弹幕] queue_id={queue_id} 忽略非二进制 WebSocket 消息：{other:?}");
                        }
                        Some(Err(e)) => {
                            log::warn!("[弹幕] queue_id={queue_id} room={real_room_id} WebSocket 接收错误：{e}");
                            return Err(format!("弹幕连接中断：{e}"));
                        }
                        None => return Err("弹幕连接已断开".into()),
                    }
                }
            }
        }
    }

    /// 广播请求序号：多队列同一刷新周期内的写入合并成一次全量广播。
    static BROADCAST_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 名单写入成功后安排一次「合并广播」，**不阻塞弹幕接收循环**。
    ///
    /// 早期版本在 `flush_tick` 分支里同步组装全量配置并逐个窗口派发事件：
    /// 一旦某个 WebView / 主线程卡住，本队列的 `ws.next()` 就不再被轮询，
    /// 表现为「计数一次后停住」，还会连带拖慢其它队列。
    /// 现在接收循环只递增一个原子序号并投递后台任务；后台任务等一小段时间
    /// 合并同批写入，只有序号最新的那次真正广播。广播失败只记日志，
    /// 绝不回流到弹幕连接或数据库写入链路。
    fn schedule_broadcast(app: &AppHandle) {
        let seq = BROADCAST_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let app = app.clone();
        // 在投递任务之前就取到 owned 的 Arc<Store>：异步块里不持有 State 借用，
        // 保证 future 满足 Send + 'static。
        let store: Option<Arc<Store>> = app
            .try_state::<Arc<Store>>()
            .map(|s| (*s).clone());
        tauri::async_runtime::spawn(async move {
            // 合并窗口：让同一刷新周期内多个队列的写入只触发一次全量推送
            tokio::time::sleep(Duration::from_millis(80)).await;
            if BROADCAST_SEQ.load(Ordering::Relaxed) != seq {
                return; // 已有更新的广播请求，交给它
            }
            let Some(store) = store else {
                log::warn!("[弹幕] 广播时 Store 状态不可用，跳过本次推送");
                return;
            };
            let config = store.get();
            log::debug!("[弹幕] 推送最新名单（第 {seq} 次广播）");
            // 弹幕只改队列名单：主窗口 + 所有 queue-* 弹窗需要刷新，
            // 字幕浮窗内容不受影响，不必骚扰。逐窗口独立发送 + 独立记录，
            // 单个窗口异常不会中断其它窗口，更不会回流到任何弹幕连接。
            for label in app.webview_windows().keys() {
                if label != "main" && !label.starts_with(crate::queue_win::LABEL_PREFIX) {
                    continue;
                }
                if let Err(e) = app.emit_to(
                    label.as_str(),
                    crate::commands::CONFIG_CHANGED_EVENT,
                    &config,
                ) {
                    log::warn!("[弹幕] 向窗口 {label} 推送名单失败：{e}");
                }
            }
        });
    }

    fn patch_status(app: &AppHandle, queue_id: i64, f: impl FnOnce(&mut QueueStatus)) {
        if let Some(manager) = app.try_state::<DanmakuManager>() {
            manager.patch(queue_id, f);
        }
    }

    fn set_error(app: &AppHandle, queue_id: i64, error: String) {
        patch_status(app, queue_id, |s| {
            s.connected = false;
            s.last_error = error;
        });
    }

    async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
        let resp = client
            .get(url)
            .header("Referer", REFERER)
            .send()
            .await
            .map_err(|e| format!("请求 B 站失败：{e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取 B 站响应失败：{e}"))?;
        if !status.is_success() {
            return Err(format!("B 站接口返回 HTTP {status}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("B 站接口返回异常：{e}"))
    }

    async fn fetch_buvid(client: &reqwest::Client) -> Option<String> {
        let value = get_json(client, BUVID_URL).await.ok()?;
        value
            .get("data")
            .and_then(|d| d.get("b_3"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    fn as_i64(value: &Value) -> Option<i64> {
        value
            .as_i64()
            .or_else(|| value.as_u64().map(|v| v as i64))
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
    }

    fn compile_patterns(patterns: &[String]) -> Vec<regex::Regex> {
        patterns
            .iter()
            .filter_map(|p| regex::Regex::new(p.trim()).ok())
            .collect()
    }

    fn is_duplicate(
        seen: &mut VecDeque<(String, std::time::Instant)>,
        key: String,
    ) -> bool {
        while seen
            .front()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(600))
        {
            seen.pop_front();
        }
        if seen.iter().any(|(k, _)| k == &key) {
            return true;
        }
        seen.push_back((key, std::time::Instant::now()));
        if seen.len() > 2000 {
            seen.pop_front();
        }
        false
    }

    /// B 站弹幕协议的二进制包头（16 字节，大端）。
    fn make_packet(op: u32, body: &[u8]) -> Vec<u8> {
        let len = 16 + body.len();
        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(&16u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // protover = 1（心跳/鉴权）
        out.extend_from_slice(&op.to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    /// 拆包 + zlib 解压；op=8 鉴权回复与 op=5 通知包原样放进 `out`。
    fn decode_frame(data: &[u8], out: &mut Vec<(u32, Value)>) -> Result<(), String> {
        let mut pos = 0usize;
        while pos + 16 <= data.len() {
            let total = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            let header_len = u16::from_be_bytes(data[pos + 4..pos + 6].try_into().unwrap()) as usize;
            let version = u16::from_be_bytes(data[pos + 6..pos + 8].try_into().unwrap());
            let op = u32::from_be_bytes(data[pos + 8..pos + 12].try_into().unwrap());
            if header_len < 16 || total < header_len || pos + total > data.len() {
                return Err("弹幕包头长度异常".into());
            }
            let body = &data[pos + header_len..pos + total];
            match op {
                5 => match version {
                    2 => {
                        let mut decompressed = Vec::new();
                        flate2::read::ZlibDecoder::new(body)
                            .take(8 * 1024 * 1024)
                            .read_to_end(&mut decompressed)
                            .map_err(|e| e.to_string())?;
                        decode_frame(&decompressed, out)?;
                    }
                    3 => return Err("弹幕服务器使用了不支持的 brotli 压缩".into()),
                    _ => {
                        if let Ok(value) = serde_json::from_slice::<Value>(body) {
                            out.push((op, value));
                        }
                    }
                },
                8 => {
                    if let Ok(value) = serde_json::from_slice::<Value>(body) {
                        out.push((op, value));
                    }
                }
                _ => {}
            }
            pos += total;
        }
        Ok(())
    }

    /// 从 `DANMU_MSG` 里取 (uid, 昵称, 文本, 毫秒时间戳)。
    fn parse_danmaku(value: &Value) -> Option<(i64, String, String, i64)> {
        let cmd = value.get("cmd")?.as_str()?;
        if !cmd.starts_with("DANMU_MSG") {
            return None;
        }
        let info = value.get("info")?.as_array()?;
        let text = info.get(1)?.as_str()?.to_string();
        let user = info.get(2)?.as_array()?;
        let uid = user.first().and_then(as_i64).unwrap_or(0);
        let nickname = user.get(1)?.as_str()?.to_string();
        let at = info
            .first()
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(4))
            .and_then(as_i64)
            .unwrap_or_else(now_ms);
        Some((uid, nickname, text, at))
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::AppHandle;

    use super::QueueStatus;
    use crate::store::QueueConfig;

    /// 非 Windows 平台不编译 B 站弹幕网络栈，保持 Linux 原生构建无需 OpenSSL。
    #[derive(Default)]
    pub struct DanmakuManager;

    impl DanmakuManager {
        pub fn reconcile(&self, _app: &AppHandle, _queues: &[QueueConfig]) {}
        pub fn statuses(&self) -> Vec<QueueStatus> {
            vec![]
        }
    }
}

pub use imp::DanmakuManager;
