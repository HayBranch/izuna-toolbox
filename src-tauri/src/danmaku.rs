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
    use std::sync::atomic::{AtomicBool, Ordering};
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

            // 1. 停掉已删除 / 已暂停 / 房间或间隔变了的任务
            if let Ok(mut workers) = self.workers.lock() {
                for (id, w) in workers.iter() {
                    if desired.get(id) != Some(&w.signature) {
                        w.cancel.store(true, Ordering::Relaxed);
                    }
                }
                workers.retain(|id, w| desired.get(id) == Some(&w.signature));
            }

            // 2. 状态表：删除已不存在的队列；未启用的显示原因
            if let Ok(mut status) = self.status.lock() {
                status.retain(|id, _| queues.iter().any(|q| q.id == *id));
                for q in queues {
                    if !q.enabled || q.room_id <= 0 {
                        let s = status
                            .entry(q.id)
                            .or_insert_with(|| QueueStatus {
                                queue_id: q.id,
                                ..Default::default()
                            });
                        s.connected = false;
                        s.last_error = if q.room_id <= 0 {
                            "未设置直播间号".into()
                        } else {
                            "已暂停监听".into()
                        };
                        if q.room_id <= 0 {
                            s.real_room_id = 0;
                        }
                        s.updated_at = now_ms();
                    }
                }
            }

            // 3. 启动新任务
            let mut spawned = Vec::new();
            {
                let workers = match self.workers.lock() {
                    Ok(w) => w,
                    Err(_) => return,
                };
                for q in queues {
                    let Some(signature) = desired.get(&q.id) else {
                        continue;
                    };
                    if workers.contains_key(&q.id) {
                        continue;
                    }
                    let cancel = Arc::new(AtomicBool::new(false));
                    spawned.push((q.clone(), signature.clone(), cancel));
                }
            }
            for (q, signature, cancel) in spawned {
                if let Ok(mut workers) = self.workers.lock() {
                    workers.insert(
                        q.id,
                        WorkerHandle {
                            signature,
                            cancel: cancel.clone(),
                        },
                    );
                }
                if let Ok(mut status) = self.status.lock() {
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
        let client = match reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(12))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                set_error(&app, queue.id, format!("初始化网络失败：{e}"));
                return;
            }
        };

        let mut host_turn = 0usize;
        while !cancel.load(Ordering::Relaxed) {
            patch_status(&app, queue.id, |s| {
                s.connected = false;
                s.last_error.clear();
            });
            let backoff = match stream_once(&app, &client, &queue, &mut host_turn, &cancel).await {
                Ok(()) => Duration::from_secs(3),
                Err(e) => {
                    set_error(&app, queue.id, e);
                    Duration::from_secs(6)
                }
            };
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            // 分片睡眠，暂停/改配置时能尽快退出
            let until = tokio::time::Instant::now() + backoff;
            while tokio::time::Instant::now() < until {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
        patch_status(&app, queue.id, |s| s.connected = false);
    }

    /// 连接一次并持续接收；连接断开时返回错误，由 run_worker 决定重连。
    async fn stream_once(
        app: &AppHandle,
        client: &reqwest::Client,
        queue: &QueueConfig,
        host_turn: &mut usize,
        cancel: &Arc<AtomicBool>,
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
        let mut pending: Vec<(i64, String, i64)> = Vec::new();
        let mut seen: VecDeque<(String, std::time::Instant)> = VecDeque::new();
        let mut connected = false;
        let mut last_message_written = 0i64;

        let flush_secs = queue.poll_secs.clamp(1, 60) as u64;
        let mut flush_tick = tokio::time::interval(Duration::from_secs(flush_secs));
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        flush_tick.tick().await; // 第一次 tick 立即返回，先吃掉

        let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        heartbeat.tick().await;

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
                            return Ok(());
                        }
                        patterns = compile_patterns(&latest.patterns);
                    }
                    if !pending.is_empty() {
                        let hits = std::mem::take(&mut pending);
                        let store = app.state::<Arc<Store>>();
                        match store.record_queue_matches(queue_id, &hits) {
                            Ok(n) if n > 0 => {
                                let _ = app.emit(
                                    crate::commands::CONFIG_CHANGED_EVENT,
                                    store.get(),
                                );
                            }
                            Ok(_) => {}
                            Err(e) => log::error!("队列 #{queue_id} 写入排队队列失败：{e}"),
                        }
                    }
                }
                msg = tokio::time::timeout(Duration::from_secs(75), ws.next()) => {
                    let msg = msg.map_err(|_| "弹幕服务器长时间无响应".to_string())?;
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
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
                                    log::info!("队列 #{queue_id} 房间 {real_room_id}：弹幕鉴权成功，开始接收消息");
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
                                    continue;
                                }
                                pending.push((uid, nickname, at));
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = ws.send(Message::Pong(payload)).await;
                        }
                        Some(Ok(Message::Close(_))) => {
                            log::warn!("队列 #{queue_id}：弹幕服务器关闭了连接");
                            return Err("弹幕服务器关闭了连接".into());
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            log::warn!("队列 #{queue_id} 弹幕连接中断：{e}");
                            return Err(format!("弹幕连接中断：{e}"));
                        }
                        None => return Err("弹幕连接已断开".into()),
                    }
                }
            }
        }
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
