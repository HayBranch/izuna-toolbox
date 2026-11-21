//! 自动更新：从 GitHub Releases 下载安装包并启动安装器。
//!
//! 分工原因：GitHub API 允许前端跨域读取（检查版本），但 release 资源下载会
//! 302 跳到**没有 CORS 头**的 `release-assets.githubusercontent.com`，前端
//! `fetch` 读不到安装包字节。因此「检查」在前端、「下载 + 安装」落在这里。
//!
//! 网络代码与 reqwest/tokio 依赖一起 gate 到 `cfg(windows)`：本应用只发布
//! Windows 版，这样 Linux 原生构建（CNB pipeline）完全不编译这些依赖，
//! windows-gnu 交叉编译也只会带上 schannel（纯 Rust，无需 C / OpenSSL）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// 取消标志：`cancel_update` 置位，`download_update` 每个数据块检查一次。
/// 用 `Arc` 包一层，方便异步命令取出一份 `'static` 的句柄跨 `.await` 使用。
#[derive(Default, Clone)]
pub struct UpdateState {
    pub cancel: Arc<AtomicBool>,
}

/// Rust → 前端的下载进度事件负载。
#[derive(Clone, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
}

/// 下载目录名（位于系统临时目录下）。
#[cfg(windows)]
const UPDATE_DIR: &str = "izuna-updates";

#[cfg(windows)]
fn update_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(UPDATE_DIR)
}

/// 点「取消下载」：只置位，真正的中止由 `download_update` 循环检查。
#[tauri::command]
pub fn cancel_update(state: State<'_, UpdateState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

/// 拉取加速节点榜单页的脚本原文（前端解析出前几个镜像）。
/// 与 `download_update` 同理：目标站没有 CORS 头，前端读不到，只能走 Rust。
/// URL 写死、不接受前端传参，避免被当作任意 URL 抓取器。
#[tauri::command]
pub async fn fetch_mirror_list() -> Result<String, String> {
    fetch_mirror_list_impl().await
}

#[cfg(windows)]
async fn fetch_mirror_list_impl() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent("izuna-toolbox-updater")
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| format!("初始化请求失败：{e}"))?;
    let resp = client
        .get("https://xiake.pro/github/script.js")
        .send()
        .await
        .map_err(|e| format!("连接节点列表站失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("获取节点列表失败：{e}"))?;
    resp.text()
        .await
        .map_err(|e| format!("读取节点列表失败：{e}"))
}

#[cfg(not(windows))]
async fn fetch_mirror_list_impl() -> Result<String, String> {
    Err("加速节点列表仅支持 Windows".into())
}

/// 下载安装包到临时目录，返回本地路径。命令在所有平台都存在，网络实现见 helper
/// （非 Windows 直接返回不支持，写法与 `commands.rs::open_url` 一致）。
#[tauri::command]
pub async fn download_update(
    app: AppHandle,
    url: String,
    file_name: String,
) -> Result<String, String> {
    // 先从状态里取出一份 Arc，State 守卫在跨 await 前就销毁，保证 future 是 'static。
    let cancel = {
        let guard = app.state::<UpdateState>();
        guard.cancel.clone()
    };
    download_update_impl(app, url, file_name, cancel).await
}

#[cfg(windows)]
async fn download_update_impl(
    app: AppHandle,
    url: String,
    file_name: String,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    use std::io::Write;
    use tauri::Emitter;

    // 入参来自前端，但只接受 https + 安全的纯文件名，避免被用来下载/写入任意位置。
    if !url.starts_with("https://") {
        return Err("下载地址必须是 https".into());
    }
    let name = std::path::Path::new(&file_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let lower = name.to_ascii_lowercase();
    if name.is_empty() || name != file_name || !(lower.ends_with(".exe") || lower.ends_with(".msi")) {
        return Err("安装包文件名不合法".into());
    }

    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let path = dir.join(&name);

    // 每次下载开始都复位取消标志：否则一次取消会让后续下载全部失效。
    cancel.store(false, Ordering::Relaxed);

    let client = reqwest::Client::builder()
        .user_agent("izuna-toolbox-updater")
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("初始化下载失败：{e}"))?;

    // 发起请求（含跟随重定向）：整体最多 10s，期间每 300ms 回头查一次取消标志。
    // 有些加速节点会「TCP 连上了却不回响应头」，裸 .send() 会永远挂起，
    // 走不到下面查取消的读循环里，取消按钮就再也不起作用（实测复现过）。
    let send_fut = client.get(url.as_str()).send();
    tokio::pin!(send_fut);
    let send_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut resp = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消下载".into());
        }
        let left = send_deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err("连接超时（10s），可换一条下载线路重试".into());
        }
        let slice = left.min(std::time::Duration::from_millis(300));
        match tokio::time::timeout(slice, send_fut.as_mut()).await {
            // 超时切片只是回头查一次取消标志，不算失败，继续等同一个请求
            Ok(r) => {
                let r = r.map_err(|e| format!("连接 GitHub 失败：{e}"))?;
                let r = r.error_for_status().map_err(|e| format!("下载被拒绝：{e}"))?;
                break r;
            }
            Err(_) => continue,
        }
    };

    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&path).map_err(|e| format!("创建安装包失败：{e}"))?;
    let mut downloaded: u64 = 0;

    let result: Result<(), String> = loop {
        // 等下一块数据：单块 15s 无数据判为网络卡死；期间同样每 300ms
        // 查一次取消标志，保证「卡住时点取消」立即生效而不是等满 15s。
        let chunk_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let chunk = loop {
            if cancel.load(Ordering::Relaxed) {
                break Err("已取消下载".into());
            }
            let left = chunk_deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break Err("下载超时（网络无响应）".into());
            }
            let slice = left.min(std::time::Duration::from_millis(300));
            match tokio::time::timeout(slice, resp.chunk()).await {
                Ok(r) => break r.map_err(|e| format!("下载中断：{e}")),
                Err(_) => continue,
            }
        };
        let chunk = match chunk {
            Ok(Some(c)) => c,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        if let Err(e) = file.write_all(&chunk) {
            break Err(format!("写入安装包失败：{e}"));
        }
        downloaded += chunk.len() as u64;
        let _ = app.emit("update-progress", Progress { downloaded, total });
    };

    if let Err(e) = result {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    file.flush().map_err(|e| format!("写入安装包失败：{e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
async fn download_update_impl(
    _app: AppHandle,
    _url: String,
    _file_name: String,
    _cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    Err("自动更新仅支持 Windows".into())
}

/// 启动安装器并退出本程序，让安装器能替换正在运行的 exe。
#[tauri::command]
pub fn install_update(app: AppHandle, path: String) -> Result<(), String> {
    install_update_impl(app, path)
}

#[cfg(windows)]
fn install_update_impl(app: AppHandle, path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    // 校验：只允许运行我们下载目录里、扩展名安全的安装包（防止被当作任意程序启动器）。
    let dir = std::fs::canonicalize(update_dir()).map_err(|e| format!("更新目录不存在：{e}"))?;
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("安装包不存在：{e}"))?;
    if canonical.parent() != Some(dir.as_path()) {
        return Err("安装包路径不合法".into());
    }
    let ext = canonical
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "exe" && ext != "msi" {
        return Err("安装包类型不合法".into());
    }

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let mut cmd = if ext == "msi" {
        let mut c = std::process::Command::new("msiexec");
        c.arg("/i").arg(&canonical).arg("/passive").arg("/norestart");
        c
    } else {
        std::process::Command::new(&canonical)
    };
    // 脱离本进程：app.exit(0) 之后安装器仍能继续运行。
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|e| format!("启动安装器失败：{e}"))?;

    app.exit(0);
    Ok(())
}

#[cfg(not(windows))]
fn install_update_impl(_app: AppHandle, _path: String) -> Result<(), String> {
    Err("自动更新仅支持 Windows".into())
}
