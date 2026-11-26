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
    const NAV_URL: &str = "https://api.bilibili.com/x/web-interface/nav";

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
            // 登录态纳入签名：切换登录/游客状态、或更换 Cookie 时，签名对比会
            // 发现变化，自动停掉旧连接并用新登录态重连，不需要额外的 force 参数。
            // 用 uid + SESSDATA 短哈希（绝不含 Cookie 明文），这样即使 Cookie 里
            // 缺 DedeUserID（uid 回退为 0），换 Cookie 也能触发重连。
            let login_sig = app
                .state::<crate::app_config::SettingsState>()
                .bili_auth()
                .map(|a| format!("{}:{}", a.uid, short_hash(&a.sessdata)))
                .unwrap_or_else(|| "guest".to_string());
            let mut desired: HashMap<i64, String> = HashMap::new();
            for q in queues {
                if q.enabled && q.room_id > 0 {
                    desired.insert(
                        q.id,
                        format!("{}:{}:{}", q.room_id, q.poll_secs, login_sig),
                    );
                }
            }
            log::info!(
                "[弹幕] reconcile：数据库队列 {} 个，需监听 {} 个（enabled 且有直播间号，登录={}）",
                queues.len(),
                desired.len(),
                if login_sig == "guest" { "游客" } else { "已登录" }
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
        let info = get_json(
            client,
            &format!("{ROOM_INFO_URL}{}", queue.room_id),
            "真实房间号",
        )
        .await?;
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
        let conf = get_json(
            client,
            &format!("{DANMU_CONF_URL}{real_room_id}"),
            "弹幕服务器配置",
        )
        .await?;
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
        // 3. 登录态：服务端只对登录连接下发真实 uid 与未打码昵称；
        //    游客连接一律 uid=0、昵称打码（如 `泉***`），无法反查真实身份。
        //    Cookie 只放在内存/独立文件里，绝不打印内容。
        //
        //    关键：B 站弹幕 WebSocket 的登录态只靠 Cookie 里的 SESSDATA 传递，
        //    鉴权包的 uid 必须保持 0（服务端依据 SESSDATA 识别账号并下发真实身份）。
        //    曾经的实现把 uid 填成 DedeUserID、Cookie 又塞进 buvid3/buvid4/sid
        //    等完整字段，与鉴权包里另取的 buvid 冲突，服务端在鉴权阶段直接 RST
        //    （表现：连接成功→发鉴权包→0 秒断开、收包=0）。这里对齐主流库做法：
        //    只发 SESSDATA、uid=0、不额外带 buvid。
        let bili_auth = app
            .state::<crate::app_config::SettingsState>()
            .bili_auth();
        let sessdata = bili_auth.as_ref().map(|a| a.sessdata.clone());
        let using_login = sessdata.is_some();
        // 房间鉴权 code=0 只代表可以进入弹幕通道，并不能证明 Cookie 有效。
        // 单独请求账号接口，明确记录 B 站是否认可这份 Cookie，避免把“连接成功”
        // 错当成“登录成功”。校验失败不阻断连接，仍保留游客弹幕和后续诊断日志。
        let login_check = if let Some(auth) = bili_auth.as_ref() {
            verify_bili_login(client, auth, queue_id, real_room_id).await
        } else {
            None
        };
        if using_login {
            log::info!(
                "[弹幕] queue_id={queue_id} room={real_room_id} 使用登录 Cookie 连接（WebSocket 仅发送 SESSDATA，鉴权 uid=0）"
            );
        } else {
            log::info!(
                "[弹幕] queue_id={queue_id} room={real_room_id} 使用游客连接（uid=0，昵称会被服务端打码；可在设置页粘贴 B 站 Cookie 登录）"
            );
        }
        // 仅游客连接需要 buvid 降低风控概率；登录连接只发 SESSDATA，避免设备指纹冲突。
        let buvid = if sessdata.is_none() {
            fetch_buvid(client, queue_id).await
        } else {
            None
        };

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
            // 登录连接只发 SESSDATA（删掉 buvid3/buvid4/sid/DedeUserID 等，
            // 避免与鉴权包字段或服务端设备指纹校验冲突而被 RST）；
            // 游客连接只带新取的 buvid 降低风控概率。
            let cookie_value = sessdata
                .as_deref()
                .map(|s| format!("SESSDATA={s}"))
                .or_else(|| buvid.as_deref().map(|b| format!("buvid3={b}")));
            if let Some(cookie) = cookie_value {
                match HeaderValue::from_str(&cookie) {
                    Ok(value) => {
                        headers.insert("Cookie", value);
                        if let Some(sessdata) = sessdata.as_deref() {
                            log::info!(
                                "[弹幕] queue_id={queue_id} WebSocket Cookie 请求头已设置：字段=[SESSDATA]，值长度={}，指纹={}（不记录明文）",
                                sessdata.len(),
                                short_hash(sessdata)
                            );
                        }
                    }
                    Err(e) => {
                        // 以前只警告后继续，实际会静默降级成游客连接。现在直接报出
                        // 本阶段错误，避免“显示已登录却收到匿名信息”却找不到原因。
                        log::error!(
                            "[弹幕] queue_id={queue_id} Cookie 请求头构造失败：{e}；本次不会降级为游客连接"
                        );
                        let field = if using_login { "SESSDATA" } else { "buvid3" };
                        return Err(format!("{field} 含非法字符，无法构造 Cookie 请求头：{e}"));
                    }
                }
            }
        }
        let (mut ws, ws_response) = tokio::time::timeout(
            Duration::from_secs(12),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| "连接弹幕服务器超时".to_string())?
        .map_err(|e| format!("连接弹幕服务器失败：{e}"))?;

        // 4. 鉴权：uid 固定 0。登录态完全由 Cookie 里的 SESSDATA 传递，
        //    服务端鉴权通过后会在弹幕里下发真实 uid 与昵称；填 DedeUserID
        //    反而会因身份/token 不匹配触发风控。
        //    buvid 仅在游客连接时带（与上面的 Cookie 头一致）。
        let auth = serde_json::json!({
            "uid": 0,
            "roomid": real_room_id,
            "protover": 2,
            "platform": "web",
            "type": 2,
            "key": token,
            "buvid": buvid.unwrap_or_default(),
        });
        log::info!(
            "[弹幕] queue_id={queue_id} WebSocket 握手成功：{host}:{port}，HTTP={}，房间={real_room_id}，protover=2，鉴权uid=0，token长度={}，buvid={}（token 不记录明文）",
            ws_response.status(),
            token.len(),
            if auth.get("buvid").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()) { "已携带" } else { "未携带" }
        );
        ws.send(Message::Binary(make_packet(7, auth.to_string().as_bytes()).into()))
            .await
            .map_err(|e| format!("发送弹幕鉴权失败：{e}"))?;
        log::info!(
            "队列 #{queue_id}：已向 {host}:{port} 发送房间鉴权包，耗时 {}ms，等待 op=8 鉴权回复",
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
        let mut identified_danmaku = 0u64;
        let mut anonymous_danmaku = 0u64;
        let login_check_text = match (using_login, login_check) {
            (false, _) => "未使用Cookie",
            (true, Some(true)) => "账号接口已确认",
            (true, Some(false)) => "账号接口判定未登录",
            (true, None) => "账号接口校验失败",
        };
        // 连接活跃度看门狗：记录最后一次收到服务端数据的时间、本次连接起点。
        // 注意不能在 select 循环里给 ws.next() 套 75s timeout——heartbeat（30s）、
        // flush、stats（60s）每隔几十秒就触发一次，每次循环都会把 timeout future
        // drop 重建，75s 永远无法到期。结果是连接假死（TCP 未断但服务端不再推
        // 数据）时既不重连也无日志，表现为"连着但收不到弹幕"。改由 heartbeat
        // 分支顺带检查空闲时长，超时主动断开让上层重连。
        let mut last_data_at = std::time::Instant::now();
        let connected_at = std::time::Instant::now();

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
                    // 看门狗：连续 90 秒没收到任何服务端帧（含心跳响应）就判定假死，
                    // 主动断开重连。B 站正常每 30 秒有心跳响应，90 秒是保守阈值。
                    // 这里必须打印明确日志，否则假死表现为"静默收不到弹幕"。
                    let idle = last_data_at.elapsed();
                    if idle >= Duration::from_secs(90) {
                        log::warn!(
                            "[弹幕] queue_id={queue_id} room={real_room_id} 连接假死：已 {}s 未收到服务端数据（收包={received_frames}，弹幕={received_danmaku}，命中={matched}，待写入={}），主动断开重连",
                            idle.as_secs(),
                            pending.len()
                        );
                        return Err(format!("连接假死：{}s 未收到服务端数据", idle.as_secs()));
                    }
                    ws.send(Message::Binary(make_packet(2, b"[object Object]").into()))
                        .await
                        .map_err(|e| format!("发送弹幕心跳失败：{e}"))?;
                    log::debug!("[弹幕] queue_id={queue_id} room={real_room_id} 已发送心跳，距离上次收包 {}s", idle.as_secs());
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
                        // 排查用：打印本批真正要写入的 UID，和"收到弹幕"日志对照即可判断
                        // 是没收到、收到没命中、还是命中了没写进去。
                        let uids: Vec<i64> = hits.iter().map(|(uid, _, _)| *uid).collect();
                        log::info!(
                            "[弹幕] queue_id={queue_id} room={real_room_id} 准备写入本批命中 {} 条，UID={uids:?}",
                            hits.len()
                        );
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
                        "[弹幕] queue_id={queue_id} room={real_room_id} 活跃统计：收包={received_frames}，弹幕={received_danmaku}（真实uid={identified_danmaku}，匿名uid0={anonymous_danmaku}），命中={matched}，重复={duplicates}，已写入={written}，待写入={}，Cookie校验={login_check_text}",
                        pending.len()
                    );
                }
                msg = ws.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            received_frames += 1;
                            // 任意服务端帧都算连接活跃，重置假死看门狗。
                            last_data_at = std::time::Instant::now();
                            log::trace!(
                                "[弹幕] queue_id={queue_id} room={real_room_id} 收到二进制帧，字节数={}",
                                data.len()
                            );
                            let mut decoded = Vec::new();
                            if let Err(e) = decode_frame(&data, &mut decoded) {
                                log::warn!(
                                    "[弹幕] queue_id={queue_id} room={real_room_id} 解析弹幕包失败：{e}（字节数={}），断开重连",
                                    data.len()
                                );
                                return Err(format!("解析弹幕包失败：{e}"));
                            }
                            for (op, value) in decoded {
                                // 排查用：把解包出来的每种消息都打印，
                                // 避免"收包很多、弹幕=0"时看不出到底收到了什么。
                                if op == 5 {
                                    let cmd = value.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
                                    log::debug!("[弹幕] queue_id={queue_id} room={real_room_id} 收到 op=5 消息 cmd={cmd:?}");
                                }
                                if op == 8 {
                                    last_data_at = std::time::Instant::now();
                                    let code = value.get("code").and_then(as_i64).unwrap_or(-1);
                                    if code != 0 {
                                        let detail = value
                                            .get("msg")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        log::warn!(
                                            "[弹幕] queue_id={queue_id} room={real_room_id} 服务端拒绝鉴权：code={code} msg={detail:?}；6 秒后重连"
                                        );
                                        return Err(format!("弹幕鉴权失败（code {code} {detail}）"));
                                    }
                                    connected = true;
                                    patch_status(app, queue_id, |s| {
                                        s.connected = true;
                                        s.last_error.clear();
                                    });
                                    log::info!(
                                        "[弹幕] queue_id={queue_id} room={real_room_id} 房间鉴权通过（op=8 code=0），开始接收消息；注意：这不代表账号 Cookie 有效，Cookie校验={login_check_text}"
                                    );
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
                                // 排查用：每条 op=5 通知都打印原始 cmd 与关键字段，
                                // 让用户能直接看到"收到的到底是什么、为什么没 +1"。
                                let cmd = value
                                    .get("cmd")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("(无 cmd)");
                                let is_danmu = cmd.starts_with("DANMU_MSG");
                                let Some((uid, nickname, text, at)) = parse_danmaku(&value) else {
                                    if is_danmu {
                                        // 收到弹幕但字段解析失败：这是"收包有、弹幕=0"的关键线索，
                                        // 打印截断的原始 JSON 便于对照 B 站协议字段变化。
                                        let raw = value.to_string();
                                        let preview: String = raw.chars().take(400).collect();
                                        log::warn!(
                                            "[弹幕] queue_id={queue_id} room={real_room_id} 收到 DANMU_MSG 但解析字段失败（info 结构变化？）：{preview}"
                                        );
                                    } else {
                                        log::debug!(
                                            "[弹幕] queue_id={queue_id} room={real_room_id} 收到非弹幕通知：cmd={cmd:?}，跳过"
                                        );
                                    }
                                    continue;
                                };
                                received_danmaku += 1;
                                if uid > 0 {
                                    identified_danmaku += 1;
                                } else {
                                    anonymous_danmaku += 1;
                                    if using_login && anonymous_danmaku == 1 {
                                        log::warn!(
                                            "[弹幕] queue_id={queue_id} room={real_room_id} 登录连接首次收到匿名身份：uid=0、昵称={nickname:?}，Cookie校验={login_check_text}。若校验已通过，说明问题发生在弹幕通道身份下发阶段，而不是 Cookie 保存阶段"
                                        );
                                    }
                                }
                                log::info!(
                                    "[弹幕] queue_id={queue_id} room={real_room_id} 收到弹幕：身份={}，uid={uid}，昵称={nickname:?}，内容={text:?}，弹幕时间={at}",
                                    if uid > 0 { "真实uid" } else { "匿名uid0" }
                                );
                                let now = now_ms();
                                if now - last_message_written >= 1000 {
                                    last_message_written = now;
                                    patch_status(app, queue_id, |s| s.last_message_at = now);
                                }
                                if nickname.trim().is_empty() {
                                    log::info!(
                                        "[弹幕] queue_id={queue_id} room={real_room_id} 跳过：昵称为空（uid={uid} 内容={text:?}）"
                                    );
                                    continue;
                                }
                                // B 站游客弹幕 uid=0：按昵称生成负数虚拟 UID 正常计数，
                                // 不再当成无效弹幕丢掉（这是"发几次后不 +1"的直接原因）。
                                let identity = if uid > 0 { uid } else { virtual_uid(&nickname) };
                                if uid <= 0 {
                                    log::info!(
                                        "[弹幕] queue_id={queue_id} room={real_room_id} 游客弹幕（uid=0），按昵称生成虚拟 UID={identity}：昵称={nickname:?}"
                                    );
                                }
                                let hit = patterns.iter().position(|re| re.is_match(&text));
                                let Some(rule_idx) = hit else {
                                    log::info!(
                                        "[弹幕] queue_id={queue_id} room={real_room_id} 未命中：内容={text:?}（当前 {} 条正则均未匹配）",
                                        patterns.len()
                                    );
                                    continue;
                                };
                                // 去重 key 不再依赖弹幕自带时间戳：B 站部分版本该字段是秒级
                                // 或缺失后回退成同值，手动连发相同文本会被误判重复、不再 +1。
                                // 改用「本队列内到达流水号」保证每条到达的弹幕都能独立计数。
                                let key = format!("{identity}:{text}:{received_danmaku}");
                                if is_duplicate(&mut seen, key) {
                                    duplicates += 1;
                                    log::info!(
                                        "[弹幕] queue_id={queue_id} room={real_room_id} 判定重复跳过：identity={identity} 内容={text:?}（重复数={duplicates}）"
                                    );
                                    continue;
                                }
                                matched += 1;
                                log::info!(
                                    "[弹幕] queue_id={queue_id} room={real_room_id} 命中 +1：规则#{rule_idx} identity={identity} 昵称={nickname:?} 内容={text:?}，待写入={}",
                                    pending.len() + 1
                                );
                                pending.push((identity, nickname, at));
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            last_data_at = std::time::Instant::now();
                            ws.send(Message::Pong(payload))
                                .await
                                .map_err(|e| format!("回复 WebSocket Ping 失败：{e}"))?;
                            log::trace!("[弹幕] queue_id={queue_id} 已回复 WebSocket Pong");
                        }
                        Some(Ok(Message::Close(frame))) => {
                            log::warn!(
                                "[弹幕] queue_id={queue_id} room={real_room_id} 服务端主动关闭连接：{frame:?}（本次连接 {}s，收包={received_frames}，弹幕={received_danmaku}，命中={matched}），6 秒后重连",
                                connected_at.elapsed().as_secs()
                            );
                            return Err("弹幕服务器关闭了连接".into());
                        }
                        Some(Ok(other)) => {
                            last_data_at = std::time::Instant::now();
                            log::debug!("[弹幕] queue_id={queue_id} 忽略非二进制 WebSocket 消息：{other:?}");
                        }
                        Some(Err(e)) => {
                            log::warn!(
                                "[弹幕] queue_id={queue_id} room={real_room_id} WebSocket 接收错误：{e}（本次连接 {}s，收包={received_frames}，弹幕={received_danmaku}，命中={matched}），6 秒后重连",
                                connected_at.elapsed().as_secs()
                            );
                            return Err(format!("弹幕连接中断：{e}"));
                        }
                        // 对端直接断开 TCP 且没有 Close 帧：以前这里静默返回，
                        // 是"连接莫名没了但日志什么都没有"的主要来源。
                        None => {
                            log::warn!(
                                "[弹幕] queue_id={queue_id} room={real_room_id} 连接已断开（对端关闭，无 Close 帧）：本次连接 {}s，收包={received_frames}，弹幕={received_danmaku}，命中={matched}，待写入={}，6 秒后重连",
                                connected_at.elapsed().as_secs(),
                                pending.len()
                            );
                            return Err("弹幕连接已断开（对端关闭）".into());
                        }
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

    /// 用完整 Cookie 请求账号接口，返回 Some(true/false) 表示接口明确给出的
    /// 登录状态；网络或响应异常返回 None。只记录状态码、code、mid 等诊断字段，
    /// 不记录 Cookie、SESSDATA、用户昵称或响应全文。
    async fn verify_bili_login(
        client: &reqwest::Client,
        auth: &crate::app_config::BiliAuth,
        queue_id: i64,
        room_id: i64,
    ) -> Option<bool> {
        let resp = match client
            .get(NAV_URL)
            .header("Referer", REFERER)
            .header("Cookie", &auth.cookie)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                log::warn!("[B站登录校验] queue_id={queue_id} room={room_id} 账号接口请求失败：{e}；本次无法确认 Cookie 是否有效");
                return None;
            }
        };
        let status = resp.status();
        let text = match resp.text().await {
            Ok(text) => text,
            Err(e) => {
                log::warn!("[B站登录校验] queue_id={queue_id} room={room_id} 读取账号接口响应失败：{e}");
                return None;
            }
        };
        if !status.is_success() {
            log::warn!("[B站登录校验] queue_id={queue_id} room={room_id} 账号接口 HTTP={status}，响应字节数={}，本次无法确认 Cookie 是否有效", text.len());
            return None;
        }
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                log::warn!("[B站登录校验] queue_id={queue_id} room={room_id} 账号接口不是有效 JSON：{e}，响应字节数={}", text.len());
                return None;
            }
        };
        let code = value.get("code").and_then(as_i64).unwrap_or(-1);
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("msg").and_then(|v| v.as_str()))
            .unwrap_or("");
        let data = value.get("data");
        let is_login = data
            .and_then(|d| d.get("isLogin"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mid = data
            .and_then(|d| d.get("mid"))
            .and_then(as_i64)
            .unwrap_or(0);
        if is_login && mid > 0 {
            log::info!("[B站登录校验] queue_id={queue_id} room={room_id} Cookie 有效：HTTP={status}，code={code}，isLogin=true，mid={mid}；本地DedeUserID={}", auth.uid);
            Some(true)
        } else {
            log::warn!("[B站登录校验] queue_id={queue_id} room={room_id} B 站未认可登录 Cookie：HTTP={status}，code={code}，message={message:?}，isLogin={is_login}，mid={mid}。请重新从已登录的 www.bilibili.com 请求中复制完整 Cookie；本次弹幕仍可能显示 uid=0/打码昵称");
            Some(false)
        }
    }

    async fn get_json(
        client: &reqwest::Client,
        url: &str,
        stage: &str,
    ) -> Result<Value, String> {
        let resp = client
            .get(url)
            .header("Referer", REFERER)
            .send()
            .await
            .map_err(|e| format!("请求 B 站{stage}失败：{e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取 B 站{stage}响应失败：{e}"))?;
        if !status.is_success() {
            return Err(format!(
                "B 站{stage}接口返回 HTTP {status}，响应字节数={}",
                text.len()
            ));
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| {
            format!("B 站{stage}接口返回非 JSON：{e}，响应字节数={}", text.len())
        })?;
        let code = value.get("code").and_then(as_i64).unwrap_or(-1);
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("msg").and_then(|v| v.as_str()))
            .unwrap_or("");
        log::debug!("[弹幕] B 站{stage}响应：HTTP={status}，code={code}，message={message:?}，响应字节数={}", text.len());
        Ok(value)
    }

    async fn fetch_buvid(client: &reqwest::Client, queue_id: i64) -> Option<String> {
        match get_json(client, BUVID_URL, "游客设备标识").await {
            Ok(value) => {
                let buvid = value
                    .get("data")
                    .and_then(|d| d.get("b_3"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if buvid.is_some() {
                    log::debug!("[弹幕] queue_id={queue_id} 已获取游客 buvid（不记录明文）");
                } else {
                    log::warn!("[弹幕] queue_id={queue_id} 游客设备标识响应缺少 data.b_3，将不携带 buvid");
                }
                buvid
            }
            Err(e) => {
                log::warn!("[弹幕] queue_id={queue_id} 获取游客 buvid 失败：{e}；将不携带 buvid 继续连接");
                None
            }
        }
    }

    /// 敏感字符串的短哈希，仅用于连接签名对比，绝不打印原文。
    fn short_hash(s: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        format!("{:x}", hasher.finish())
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
                        let value = serde_json::from_slice::<Value>(body)
                            .map_err(|e| format!("op=5 通知正文不是有效 JSON：{e}"))?;
                        out.push((op, value));
                    }
                },
                8 => {
                    // 鉴权回复解析失败以前会被静默忽略，随后 op=5 又可能把状态
                    // 标成已连接，排错时完全看不到真正的协议异常。
                    let value = serde_json::from_slice::<Value>(body)
                        .map_err(|e| format!("op=8 鉴权回复不是有效 JSON：{e}"))?;
                    out.push((op, value));
                }
                _ => {}
            }
            pos += total;
        }
        if pos != data.len() {
            return Err(format!("弹幕包末尾有 {} 个无法解析的字节", data.len() - pos));
        }
        Ok(())
    }

    /// B 站游客/未登录弹幕的 uid 是 0、昵称被打码（如 `泉***`），
    /// 但这是真实弹幕，不能丢。用昵称生成一个稳定的**负数**虚拟 UID：
    /// - 负数避免和真实 UID（>0）冲突；
    /// - 也不用 0，因为 0 被「手动添加成员」占用；
    /// - 同一个打码昵称映射同一个值，能正常累加次数；
    /// - FNV-1a 哈希，纯计算，跨重启稳定。
    fn virtual_uid(nickname: &str) -> i64 {
        let mut hash: u64 = 0xcbf29ce4_8422_2325;
        for &b in nickname.trim().as_bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        // 取低 62 位再取负，保证结果 <= -1 且不会溢出。
        -((hash & 0x3fff_ffff_ffff_ffff) as i64).max(1)
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
